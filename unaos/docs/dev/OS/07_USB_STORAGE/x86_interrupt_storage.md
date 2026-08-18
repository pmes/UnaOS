# STOR-D1 — IF-safe interrupt-driven x86 storage (design)

Status: **S1–S7 LANDED + ✅ METAL-CONFIRMED; S8 LANDED (QEMU-green, metal PENDING)** (S1–S4 2026-07-10,
S5–S6 2026-07-11, S7 2026-07-12, S8 2026-07-15, `hw-rmbp`, behind the `irqstorage` knob). S8 adds writable
dynamic on-disk files (overwrite-only) — its `S8-write` witness is QEMU-green and awaits the attended rMBP
bench (round6-rmbp.spec PENDING). The WHOLE S1–S7 chain rode one knob-ON usbdebug boot on the real 2012 rMBP at
the round-9 attended bench (2026-07-12, TWO clean boots): `./arroyo mbench` PASS 37/37 required + 0 forbidden
+ 6/6 pending matched, 0 fault — S6's `S6-witness` (locked 240000/240000 intact, unlocked lost 119996/240000
under real cross-core contention), S7's `S7-openany` (README.TXT resolved dynamically + `owned.bin` refused),
S4's `S4-race` (close-release + teardown-release synchronous delete) + `S4-mf2`, and S1–S5's `bx-blockreq`/
`S3`/`S4`/`S5` all fired on silicon (S1–S5 first metal-confirmed round-6 2026-07-11; S1–S3 core mechanism
`7b2f05f`/`b73ba08`/`fd5de85`). S1 the
storage service task + `BlockRequest` submit/block/complete; S2 live in-place reads (`sys_read`); S3 live
in-place write-through (`sys_write_file`) closing the close-discards-dirty residual; **S4 synchronous
grow/create/delete in-syscall** (`BlockOp::{Create,Grow,Delete}`), retiring the U10x deferred op-queue +
its launcher-replay causal-fidelity gap when the knob is on; **S5 real shared backing for cross-process
created-file reads** — a created descriptor's `sys_read` now reads the LIVE on-disk volume BY NAME
(`created_read_live` → `submit_read_file`) instead of a private wstage snapshot, `open_created_sibling`
stops snapshot-copying (empty seed knob-on), and `sys_open_dynamic` re-checks `DYN_DELETED_G` after
resolving — closing the U11x M2 torn-copy (residual 3) + open-vs-unlink TOCTOU (residual 4) **when on**.
S4/S5's cross-process delete-at-last-close + shared-backing races (u11m2/u6gx) are metal-only-validatable
(risk 3); S4's highest-risk edit — a blocking last-close delete on the most load-bearing lifecycle
primitive — is made safe by a runtime blocking-safe-teardown check (`current_user_cr3() != slot_cr3(slot)`;
§4 step S4). **S5 SCHEDULING NOTE:** routing created reads through the single service task exposed a real
deadlock — a NON-preemptible ring-3 fixture busy-spinning (IF=0) on the service task's core starves it, so a
cross-core created read blocking on the service task never completes. Fixed by (a) spawning the service task
`PRIO_HIGH` (a system service other tasks block on must preempt spinning user tasks — a same-priority wake
only "waits its turn", `poke_for`), and (b) making u6gx's cooperative-spin fixtures preemptible knob-on
(gated on `s4_sync_storage()`; the timer can then evict the spinner). See §4 step S5 + §5 risk 4. **S6 the
syscall-layer NAMESPACE lock** (the pi4 F3 twin, `syscall.rs`) makes the created-file open/create/unlink name
sequences MUTUALLY ATOMIC, closing S5's non-atomic source-resolve + re-validate residual AIRTIGHT — held for the
O(1) in-memory namespace decision only, with the blocking `submit_create`/`submit_delete` lifted OUT of the lock
(the S5 deadlock class stays closed). §6 decision 2 is RESOLVED: `FAT_MUTATION` is NOT activated on x86 (vacuous
under the single-service-task-writer invariant); `fat.rs` stays untouched. **S7 the arbitrary-file open**
(2026-07-12, `syscall.rs` + `irqstorage.rs`) retires the last U6bx staged-set constraint: `sys_open` of a
PRE-EXISTING on-disk file that is neither staged nor a U10 name resolves DYNAMICALLY through the service task
(a new `BlockOp::Stat` sizes it, reads route live BY NAME) — READ-ONLY (no write path to arbitrary files, MF2
generalized), U10 names excluded (they keep their owner-ACL semantics), no `ns_lock` (a dynamic file is outside
the U10 mutation namespace). **The whole S1–S7 chain has now landed** behind the `irqstorage` knob (QEMU-green,
metal-pending). The original design (below) is unchanged. Track: `hw-rmbp` (x86_64, 2012 rMBP).
Twin reference: the aarch64 polled storage path, which this design brings x86 into semantic parity with
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
| **S4** ✅ | **Route grow/create/delete synchronously** (in-syscall `fat.rs` calls via the service task — `submit_create`/`submit_grow`/`submit_delete`; serialization is by the SINGLE service-task BOT owner, **not** `FAT_MUTATION`, which stays an S6 no-op passthrough on x86). | U10x `U10_*` op-queue + launcher replay (causal-fidelity residual) | `u10x-grow`/`u10cx-create`/`u10dx-delete`: on-disk change visible **within the syscall**, no deferred drain |
| **S5** ✅ | **Real shared backing for cross-process created-file reads** (read the live on-disk volume BY NAME, not a wstage snapshot). `created_read_live`→`submit_read_file`; `open_created_sibling` seeds EMPTY knob-on (no snapshot copy) + re-validates the source names the caller's nameid (recycle fail-closed); `sys_open_dynamic` re-checks `DYN_DELETED_G` after resolving. | U11x M2 residual 3 (torn-copy/disclosure) + residual 4 (open-vs-unlink TOCTOU) — **closed-when-on** (recycle-to-wrong-file leg narrowed; airtight = S6 lock) | `s5_shared_backing_witness`: a cross-row sibling with an EMPTY private wstage reads a peer's POST-OPEN overwrite from the live backing; u11m2/u6gx cross-process reads now serve live shared backing. **Scheduling:** service task `PRIO_HIGH` + u6gx fixtures preemptible knob-on so the service task is not starved by a non-preemptible same-core spinner (§5 risk 4) |
| **S6** ✅ | **The syscall-layer NAMESPACE lock** (`syscall.rs`, the pi4 F3 twin) — an IRQ-masked `SpinMutex` making the three created-file name sequences (sibling-open `created_desc_any_row`+re-check+ACL+`open_created_sibling`; `open_create_new`; the `sys_unlink` sweep) MUTUALLY ATOMIC. Held for the O(1) in-memory namespace decision ONLY — the blocking disk `submit_create`/`submit_delete` are lifted OUT (before/after the lock), so the S5 deadlock class stays closed. **NOT `FAT_MUTATION` (§6 decision 2 = keep the x86 passthrough — vacuous under the single-writer invariant).** | S5 residual 1 (non-atomic source-resolve + re-validate → recycle/open-vs-unlink/reincarnation windows) — **closed airtight** | `s6_witness_launcher` (`S6-witness:` line, uncounted): a cross-core in-RAM RMW on the SAME `ns_lock` — LOCKED reaches `2*N` intact, the UNLOCKED control loses under contention (positive proof the lock holds; true-parallelism metal-latent per risk 3) |
| **S7** ✅ | **Retire the U6bx staged-open constraint** — `sys_open` of a pre-existing on-disk file that is neither staged nor a U10 name falls through to DYNAMIC on-disk resolution (`BlockOp::Stat` sizes it via `find_located`; reads route live BY NAME through `submit_read_file`). READ-ONLY (`-EACCES` on a writable open — MF2 generalized: no write path to arbitrary files); staged/U10 names EXCLUDED **case-insensitively** — the name is canonicalized to 8.3 UPPERCASE before the `staged_lookup`/`u10_name_id` exclusion, because `find_located` matches case-insensitively (else `owned.bin` would bypass the U6gx owner ACL and read the private `OWNED.BIN` — security-review CONFIRMED-and-fixed, `eq_name`'s equal-length rule makes uppercasing a complete defense; a closed created file stays `-ENOENT` in any casing); no `ns_lock` (outside the U10 mutation namespace). The pre-stage buffer is RETAINED (knob-off staged reads + `sys_spawn`'s program image), no longer the knob-on open boundary. | the BSP-staged set constraint (U6bx) | `s7_openany_witness`: opens README.TXT (non-staged, non-U10) via the REAL `sys_open_dynamic`, proves a DYNAMIC descriptor sized from the live volume + a live read BY NAME returns the known prefix (`:: S7-openany: … witness OK ::`, uncounted) |
| **S8** ⏳ (metal pending) | **Writable dynamic on-disk files, OVERWRITE-ONLY** — `open_dynamic_ondisk` honors `mode` bit0, so a RW open of a non-staged/non-U10 on-disk file gets `CAP_READ｜CAP_WRITE` (no wstage — a dynamic descriptor never stages). `sys_write_file` gains a dynamic branch keyed on `FILE_DYNLEN != 0` (BEFORE the `FILE_WSTAGE == 0 → EIO` guard): CAS-claim `[offset, offset+want)` with `want = min(len, FILE_SIZE − offset)`, validate the whole user source, then bounce page-at-a-time through a kernel `[u8; PAGE_SIZE]` to `submit_write_file` BY NAME (`dyn_write_live` — the `created_read_live` write twin). Strictly overwrite-only: a write AT/PAST EOF returns 0 (never grows — `write_at` allocates no clusters, touches no directory), so the on-disk SIZE is immutable for the boot even as CONTENT changes. ANY submit error / short chunk fails the WHOLE write `-EIO` (never a masked partial). No `ns_lock` (outside the U10 mutation namespace). **A deliberate widening** — any ring-3 task with `sys_open` can overwrite any arbitrary on-disk file; MF2 stays closed by EXCLUSION (staged/U10 names dropped case-insensitively at open) and the ACL surface is unchanged (dynamic files never enter `OWNED_FILES` → public-by-default, mirroring S7 reads + created-file policy). | S7's read-only constraint | `s8_write_witness`: RW-opens S8W.BIN (64 B of 0xA5, non-staged/non-U10), proves `CAP_WRITE` + `FILE_DYNLEN != 0` + size 64, overwrites a 16-byte pattern live at offset 8, reads it back, RESTORES the seed (pristine + idempotent); negatives — a past-EOF write returns 0, and a case-variant of staged code (`hello.bin`) RW is refused (`:: S8-write: … witness OK ::`, uncounted) |

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

   > S5 honesty fold-in: S5 has **no residual read/write race to defer to metal** — the torn-copy is
   > closed *by construction* (the snapshot COPY is removed, `open_created_sibling` seeds empty), and all
   > file I/O serializes through the single service-task BOT owner, so a read and a write to the same file
   > can never interleave (architectural, not a TCG artifact). The S5 witness proves the read SOURCE is the
   > live shared backing (a sibling with an EMPTY private wstage reads a peer's post-open overwrite) — a
   > deterministic before/after discriminator, not a race the metal bench must re-run.

4. **Service-task starvation by a non-preemptible same-core spinner (found + fixed in S5, QEMU-reproducible,
   NOT metal-only).** Routing created reads through the single service task makes a read BLOCK on it. A
   ring-3 task spawned NON-preemptible (`spawn_user_in_space`, IF=0) that busy-spins — e.g. u6gx's owner A
   parked on its cooperative GO word — on the service task's core (both take `online.first()` = cpu 1)
   makes the core UNSCHEDULABLE, so the service task never runs and a cross-core created read (u6gx's
   grantee B) blocks forever: a deadlock (launcher waits B → B waits the service task → the service task
   waits behind A's IF=0 spin → A waits the launcher). No priority preempts an IF=0 task. **Fix (landed):**
   (a) the service task is spawned `PRIO_HIGH` — a system service other tasks block on must out-rank a
   spinning user task so a cross-core wake sends the preempt IPI (`poke_for`: `prio > running`; a
   same-priority wake only "waits its turn"); (b) u6gx's cooperative-spin fixtures are spawned PREEMPTIBLE
   (`spawn_user_preemptible`) knob-on (gated on `s4_sync_storage()`; knob-off keeps the byte-identical
   non-preemptible spawn), so the timer can evict A and the `PRIO_HIGH` service task runs. **Metal
   relevance:** a well-behaved program blocks/yields; a *malicious* busy-spinner on the service core could
   still DoS created reads on this single-service-task architecture — a broader scheduler-fairness concern
   (a yield syscall, a dedicated service core, or per-core service tasks) noted for future work, out of S5's
   scope. Ledgered in SECURITY.md (STOR-1 S5).

---

## 6. Decisions the seat / Peter must make

1. **IRQ-context wakeup policy.** Keep the MSI-X handler strictly wake-from-`hlt` (recommended — no
   `make_ready` in interrupt context, matches the existing scheduler invariant `sched.rs:28`), or allow
   a direct `Semaphore::post` from the handler for lower completion latency? The recommended path costs a
   scheduling hop (IRQ wakes the core → service task runs → posts submitter) but keeps interrupt context
   lock-free. **Recommendation: wake-only.**

2. **`FAT_MUTATION` weight on x86** (§3.3). **RESOLVED (S6, 2026-07-11): do NOT activate `FAT_MUTATION` on
   x86 — keep `with_fat_lock` the documented zero-cost passthrough.** It is VACUOUS given the
   single-service-task-writer invariant: there is exactly ONE BOT writer (the service task), so the pi4
   FAT lost-update RMW race is not reachable on x86 — the lock would guard a race that cannot occur. The
   real S5 residual is at the SYSCALL layer (namespace atomicity of the created-file open/create/unlink
   sequences), NOT the FAT-sector RMW, so S6 landed a syscall-layer `NAMESPACE` lock (`syscall.rs`, the pi4
   F3 twin) instead. This keeps `fat.rs` (shared kernel-core, off the rmbp lane's free edit) untouched.
   (Activating `FAT_MUTATION` would need a seat call + the pi4 K-track ccd, and buys nothing here.)

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

## 8. SINGLE FAT WRITER (2026-07-26) — the flight recorder is no longer a second mutator

**Root cause (A/B-proven).** `fs/fat.rs`'s `with_fat_lock` / `with_dir_lock` are deliberately INERT on
x86 (masking IRQs across the `hlt`-driven xHCI BOT pump would hang the core — §5), and their comments
used to claim x86 had a "single FAT writer by construction". That was FALSE. The FLIGHT RECORDER
(`flight_recorder.rs::service()`, called from the x86 main loop on the BSP) re-created `UNAOS.LOG` on a
main-loop cadence — `delete_located` (free ~32 clusters) + `create_in_root` (a root-dir-sector RMW) +
`write_grow` (re-allocate + re-chain) — CONCURRENTLY with the demo chain's writers on the AP cores.
Symptoms: cross-linked chains (`GROW.BIN` chain length 5/6 where 2 was expected) and delete-witness
first-free snapshots allocated out from under the verdict. Evidence: recorder stubbed out → 0/3 FAIL;
recorder on → 3/3 FAIL on `UNAOS_HUBSTORAGE=1 ./arroyo test-fat sf 300`. BOT transport was clean
(timeouts=0, ~730x headroom) — the fault was allocator concurrency, not the transport.

**Fix — reserve once, then write in place.** The recorder is removed from the set of FAT mutators
rather than being serialized against them:

1. **RESERVE, once.** On the first main-loop pass where `block::info()` is `Some`, the recorder makes
   `UNAOS.LOG` exist at a fixed `RESERVE_BYTES` (= `RING_CAP` + 512 = 66048). This is its ONLY lifetime
   FAT/directory mutation, and it is exclusive **by construction**: the `flight_recorder::service()`
   call site precedes every `U*_probe_once()` fixture/launcher spawn in the same main-loop iteration at
   BOTH `main.rs` loop sites, and every other x86 FAT writer gates on the same `block::info()` that has
   only just become `Some`. A file already ≥ `RESERVE_BYTES` from a previous boot is reused untouched
   (zero mutation at all).
2. **Write IN PLACE, forever after.** Every later flush is a single `FatFs::write_at` over the reserved
   chain — strictly bounded to clusters already in the file, never allocating/freeing a cluster, never
   writing a FAT entry, never touching a directory sector (`fat.rs` `write_at` contract). It therefore
   cannot interact with any other writer at all.

**Why not "route the recorder through the storage service task".** That would only close the race with
`irqstorage` ON. The demo-chain writers are the `witness`-gated U10x op drains (`u10_drain_*`), which
mutate the FAT directly from an AP launcher task with the knob OFF — which is exactly the configuration
of the gate that was failing 3/3. The reserve-then-write-in-place shape is knob-agnostic and strictly
stronger: after bootstrap there is nothing left to serialize.

**Cost.** `UNAOS.LOG` is always 66048 bytes on disk: the captured log as its prefix, an explicit
`:: FLIGHTREC: end of log (N captured byte(s); the remainder of this 66048-byte file is reserved
padding) ::` marker, then zero padding. The reserve is one bounded 64 KiB zero-fill at boot.

**Witnesses.** `:: FR: UNAOS.LOG reserved 66048 bytes @cluster N reused=B — flushes are
write-in-place only (single FAT writer preserved) ::` (once, at bootstrap) and
`:: FLIGHTREC: boot log -> UNAOS.LOG (N captured bytes into a 66048-byte in-place write) -> PASS ::`
(once, first successful flush). FRWRITE (2026-07-26): `reused=` says whether the reservation reused a
previous boot's file. Only that case leaves a stale tail, so only that case pads the first flush out
to `RESERVE_BYTES` — the create/grow cases just zero-filled the file, and padding it again cost ~129
READ(10) + ~129 WRITE(10) single-sector BOT transactions. See
[`usb_xhci.md`](../../../../../docs/dev/OS/07_USB_STORAGE/usb_xhci.md) §12.

**The x86 invariant, stated honestly** (now in `fat.rs`'s `with_fat_lock` / `with_dir_lock` non-aarch64
doc comments, replacing the false claim): x86 has no in-`fat.rs` serialization. Consistency is a caller
discipline — knob-on, the ONE storage service task runs every create/grow/delete one at a time at IF=1;
knob-off, the mutating callers are sequenced one-shot fixtures and the interactive shell; and the
recorder is not a mutator after boot. A NEW concurrent x86 FAT mutator must join one of those schemes.

## 9. x86 FAT concurrency audit (2026-07-27) — the mutator roster and what actually serializes it

§8 fixed the recorder and then *scoped out* the obvious follow-up question: the x86 consistency
invariant is now pure caller discipline, and nobody had checked whether that discipline holds. This
section is that check. It enumerates **every** x86 path that allocates or frees a cluster, writes a FAT
entry, or read-modify-writes a directory sector, and names what serializes each against the others.

### 9.1 The roster

| # | Mutator (call sites) | Primitives | Context / core | Gate | Serialized against the rest by |
|---|---|---|---|---|---|
| 1 | `flight_recorder::reserve_log` (`flight_recorder.rs`) | `delete_located`, `create_in_root`, one `write_grow` | BSP main loop, IF=1, **not a scheduled task** | x86 always; **one-shot per boot** (`RESERVED` latch) | **Program order.** `flight_recorder::service()` precedes every `U*_probe_once` and `install_probe_once` at BOTH `main.rs` loop sites, and all of them gate on the same `block::info()` that has only just become `Some`. The reserve completes synchronously before any launcher is spawned. |
| 2 | `flight_recorder::write_log` | `write_at` only | BSP main loop | always | **Not a mutator.** Bounded to clusters already in the reserved chain — no FAT entry, no directory sector. Cannot interact with any writer. |
| 3 | `service_create_file` / `service_grow_file` / `service_delete_file` / `service_write_file` (`drivers/xhci/irqstorage.rs`) | `create_in_root`, `write_grow`, `delete_located`, `write_at` | `storage-svc` task, AP[0] (`online_aps().first()`), **PRIO_HIGH** | `irqstorage` | **One task, one request at a time** off `REQ_QUEUE`; every submitter blocks on the request's `done` semaphore until it completes. A real single writer for everything routed through it. |
| 4 | `u10x_preflight_grow_file`, `u10_preflight_absent` (`arch/x86_64/syscall.rs`) | `delete_located`, `create_in_root`, `write_grow` | the `u6bx-launch` → `u7x-launch` → … LAUNCHER task, AP[1], PRIO_NORMAL | `witness` **and** `HELLO_STAGED` | **Program order on one task.** The launcher chain is a linear sequence of tasks each spinning on the previous stage's `*_LAUNCH_DONE`; each stage mutates only *before* it spawns its fixture. |
| 5 | `u10_drain_grow` / `_create_grow` / `_create_grow_delete` / `_delete`, `flush_drain_one` | same set + `write_at` | same launcher task | `witness` (U10 queue is empty knob-on) | ditto — reached only after the launcher has observed its fixture's teardown (`cleared`). |
| 6 | `openf_release`, `u11m2_phase` → `submit_grow` / `submit_delete` | via row 3 | launcher task / fixture teardown | `irqstorage` | Submitters, not mutators — they funnel into row 3. |
| 7 | **`shell::dispatch_command`** (`shell.rs`, 23 mutating sites) | `create_in_dir`, `create_dir`, `write_grow`, `delete_located`, `remove_dir`, `rename_entry`, `move_entry`, plus a raw `write <lba> <byte>` | **BSP GUI main loop, INLINE** (`main.rs`'s `handle_key`) — not a scheduled task, and **not** routed through row 3 even when `irqstorage` is on | **none — compiled unconditionally** | **Nothing.** |
| 8 | `install::write_sectors` (`install/mod.rs`) | raw GPT / FAT32 format sector writes | BSP main loop | `installdemo` | **Program order** on the BSP loop, plus configuration exclusion: its blank scratch disk leaves `HELLO_STAGED` false (rows 4–5 skip their FAT legs) and permanently fails row 1's reserve. |

Non-mutators confirmed read-only and therefore off the roster: `fat::probe_once`, `selftest.rs`'s
`tste` storage section (explicitly "READ-ONLY toward storage"), `install`'s read-back verify, and every
`find_located` / `read_dir` / `read_at` / `first_free_cluster` walk.

### 9.2 Verdict

**Rows 1–6 and 8 hold. Row 7 — the shell — is a real second-writer window that is currently closed only
by configuration, not by structure.**

The evidence for rows 1–6/8 is the sequencing above: one BSP main loop running the recorder and the
installer in program order ahead of every launcher spawn; one launcher task carrying the whole demo
chain in program order; one storage service task draining one request at a time. Nothing in that set
mutates the FAT at the same time as anything else in it.

The shell is different. `shell::dispatch_command` calls `fs/fat.rs`'s mutators **directly**, from the
BSP GUI main loop, with no feature gate and no route through the storage service task. The breaking
interleave is concrete:

1. BSP main-loop iteration N: `u6bx_probe_once()` spawns the launcher chain on AP[1].
2. Same iteration, a few lines later: the event drain pops a queued `Event::Key(b'\n')` and calls
   `handle_key` → `dispatch_command("rm GROW.BIN")` → `fs.delete_located(...)` → `free_chain` →
   `set_fat_entry` on the BSP.
3. Concurrently on AP[1] (or, knob-on, on the `storage-svc` task at AP[0]): `write_grow` →
   `alloc_cluster` → compare-and-claim → `set_fat_entry_inner` on the same FAT sector.
4. The two read-modify-writes of that sector interleave. One update is lost; the loser's chain is
   cross-linked into the winner's — the exact §8 signature.

**Why it has never fired.** (a) The QEMU batteries are headless (`-display none`): QEMU's `usb-kbd` is
attached but no key event is ever produced, and the only `handle_key` feed on x86 is the HID event
queue (`main.rs`'s `poll_input()` drain is `#[cfg(target_arch = "aarch64")]`), so `dispatch_command`
is unreachable there. (b) Shipping GUI/media builds are DEFAULT-QUIET: `witness` is off, so rows 4–6
do not exist and the shell is genuinely the only writer. The window opens only in an attended GUI boot
built **with** `witness` (and/or `irqstorage`), where a keystroke lands while the launcher chain is
mid-grow.

**Secondary (degenerate) window, recorded for completeness.** Every launcher handoff is a *bounded*
spin (`ticks() + N` on `*_LAUNCH_DONE`, on the fixture's `*_DONE`, and on `cleared`). On expiry the
next stage proceeds anyway and may mutate while the previous stage is still alive. This is reachable
only on a path that already prints `FAIL`, so it degrades a diagnosis rather than a passing run.

### 9.3 Proposed minimal fix for row 7 — NOT applied here (out of lane)

Do not make `with_fat_lock` real on x86: masking IRQs across the `hlt`-driven BOT pump would hang the
core (§5, and the `fat.rs` doc comments). The minimal shapes, cheapest first — each needs `main.rs`
and/or `shell.rs`, i.e. a lane widening:

1. **Gate the shell's write verbs on the chain being done** (smallest diff): have the GUI loop's
   `handle_key` refuse the mutating verbs until the last launcher has released its `*_LAUNCH_DONE`.
   Closes the window in the only configuration where it exists; costs the attended `witness` GUI boot
   a few seconds of read-only shell.
2. **Route the shell through the storage service task** when `irqstorage` is on. Correct for the
   knob-on case but does nothing knob-off — the same objection §8 raised against routing the recorder.
3. **A `fat.rs` namespace/volume lock** spanning the composite scan→mutate sequences, taken by a
   context that can block. This is the eventual right answer (the aarch64 side already ledgers the same
   residual under FATDIRS invariant 5), but it is a real design arc: the BSP GUI loop is *not* a
   scheduled task, so it cannot block on a `Mutex` — the shell would first have to become one.

### 9.4 The tripwire that landed

`fs/fat.rs` now carries the roster as documentation, plus a `witness`-only, zero-cost-when-off
tripwire around the two seams that are real locks on aarch64: `with_fat_lock` (FAT-sector RMW) and
`with_dir_lock` (directory-sector RMW). Entering either with a nonzero in-flight count means a second
concurrent mutator, and it bumps a sticky counter and emits ONE line:

```
:: FATRACE: SECOND CONCURRENT x86 FAT-table MUTATOR DETECTED — the caller-side single-writer roster
   (see fs/fat.rs `with_fat_lock`) has been violated; expect cross-linked chains ::
```

This is sound because those two spans provably do not nest: `with_fat_lock` is taken only by
`set_fat_entry` and `alloc_cluster`'s compare-and-claim, and the claim body calls the lock-free
`set_fat_entry_inner` (a factoring that exists because the aarch64 lock is non-reentrant, so a nest
would deadlock *there* — the invariant is enforced by aarch64, not assumed); `with_dir_lock`'s six
bodies are pure single-sector RMWs that never call one another, and `create_in_dir` dispatches to
`create_in_root` before taking the lock. Knob-off the whole family is `#[cfg]`-compiled out and the
lock helpers are the byte-identical `#[inline(always)]` passthroughs. aarch64 is untouched — it holds
real locks, and this is an x86-only invariant. The tripwire never prints in a correct run, so it cannot
perturb a battery verdict; `fat::x86_rmw_overlaps()` reports the `(FAT, directory)` tallies, and `(0, 0)`
is the positive evidence that the discipline held for every RMW that actually ran.
