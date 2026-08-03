# xHCI controller concurrency — the claim/loan model (WEDGE-8 / F3)

Status: landed on `hw-pi4` (both arches). QEMU cannot reproduce the defect this fixes
(`timer_preempt` never runs on raspi4b, so nothing preempts a lock holder); metal verification
needs a fleet with at least one vug launched via `run` plus FAT traffic on the `Usb` source.

## The defect (F3, one of the F1–F4 masked-spinner family)

A masked span blocks on a lock that a *preemptible* task holds; the holder is preempted mid-hold;
tasks never migrate and pinned tasks are never stolen, so the holder never runs again; the masked
spinner can then take no timer IRQ, so that core dies silently — no panic. There is no ABBA cycle
anywhere in this family; lock-ordering discipline fixes none of it.

F3's instance: `XHCI_CONTROLLER` was held straight across `pump_until_bot_done`, whose wall-clock
budget is `hw_wait_budget() * 3` ≈ 8.3 s on the Pi (450 M CNTVCT ticks at 54 MHz) against a 12 ms
scheduler quantum — mid-hold preemption was a certainty, not a race. Holders: `pump_usb_into_gui`
(the ~4 ms input-service pass, whose `service_storage` runs the whole SCSI bring-up), and the block
layer's per-sector BOT calls. Masked acquirer: EL0 `SYS_WRITE` → `fat.rs` `without_interrupts`
(FAT/dir RMW) → `drivers/block.rs`. The spinner holds `FAT_MUTATION` throughout, so the filesystem
dies with the core. At 8.3 s the hold also sits just under `[spin1]`'s 10 s threshold, so a single
BOT timeout left no witness.

Note the distinction the fix preserves: the *masked 8.3 s WFI inside a FAT RMW on the `Usb` source*
is a bounded stall, not a deadlock (WFI wakes on a pending IRQ even masked; the CNTVCT deadline
retires it). The deadlock was the *unmasked holder* path. They are different problems; only the
second one is fixed here, and the first is unchanged and still documented on `with_fat_lock`.

## The fix — both halves

F1's fix (WEDGE-7) masked TABLE's critical sections, affordable because they are bounded row scans.
An 8.3 s BOT pump can never be masked, so WEDGE-8 applies the same discipline to the **lock**, not
the **work**:

**Holder half — claim/loan.** `XHCI_CONTROLLER` is now a *private*
`Mutex<Option<Box<XhciController>>>` in `drivers/xhci/mod.rs`. The only operations on it are
`claim()` / loan-drop / `install()`, each a masked O(1) `Box` take/put (mask taken before the
acquire, lock released before the unmask — the WEDGE-7 guard order). All use of the controller
happens on the claimed loan (`XhciLoan`, `Deref`/`DerefMut`, RAII return), so the long BOT work
runs with **no lock held**. A contender is told `Busy` immediately instead of spinning:

* pump/service passes (`pump_usb_into_gui`, the x86 GUI loop, `pal::pump_and_poll`, the tegra
  pumps) skip the pass and retry milliseconds later;
* the block layer surfaces `BlockError::Busy` (below);
* diagnostics (`usbinfo`/`diskinfo`, the one-shot topology summary) report "busy" honestly.

**Masked half — fail fast, retry outside the mask.** `drivers/block.rs::claim_xhci_for_io` refuses
to wait when IRQs are masked (`arch::irqs_masked()`) and returns `Busy` instantly; unmasked callers
keep effectively-blocking semantics, honestly bounded (`hlt` retries up to `hw_wait_budget()`).
`fat.rs` keeps `Busy` distinct from `Io` end to end; the FAT/dir RMW wrappers
(`with_fat_lock_src`/`with_dir_lock_src`) retry the whole closure **outside** the
`without_interrupts` span — bounded by `RMW_BUSY_ATTEMPTS` (64) *and* `hw_wait_budget()` wall-clock,
with a `hlt` between attempts so the scheduler can run the loan holder. Exhaustion surfaces
`-EAGAIN` at the syscall boundary (`SYS_READ`/`SYS_WRITE`/grow/create/mount) and `-EAGAIN` from the
shell's errno tagger: nothing was mutated, the caller may retry. Paths that map a residual `Busy`
to `-EIO` (some dir verbs) are coarse but honest — they fail cleanly after the bounded retries,
never hang.

## Invariants (standing rules for future arcs)

1. **The `XHCI_CONTROLLER` mutex is held only inside `claim`/loan-drop/`install`** — masked, O(1),
   no print, no alloc, no I/O. Enforced by the compiler (the static is private) and checkable by
   grep: `XHCI_CONTROLLER.lock()` appears exactly three times, all in `drivers/xhci/mod.rs`.
2. **No `without_interrupts` span in `fs/` ever blocks on a driver lock.** A masked context asks
   once (`claim`, O(1)) and takes `Busy` for an answer; waiting happens unmasked, outside the span.
3. **No task-preemptible context holds any lock across `pump_until_bot_done`.** The pump runs on
   the loan; the loan is ownership, not a lock — nobody spins on its holder.

## What a `Busy` costs in practice

The loan is held for the duration of one service pass (µs–ms), one healthy BOT transaction (ms), or
— worst case — one *failing* transfer's full deadline ladder (8.3 s per stage). The unmasked
bounded waits (~2.8 s aarch64 / ~2 s x86) cover the first two entirely; only the pathological third
surfaces `Busy`/`-EAGAIN` to callers, which is precisely the case that used to deadlock a core.

## The family and its two idioms

WEDGE-8's shape is no longer one arc's fix. Three locks in this tree now carry the F1–F4 discipline
(`wm::TABLE`, `XHCI_CONTROLLER`, the mailbox transport) and a fourth (`cursor::SPRITE`) is mid-
conversion — and they carry it in **two** idioms. That is not an inconsistency left behind by three
authors; it is the choice the discipline actually asks of every lock it is applied to.

The invariant is the same in both cases, and it is the one the family's defect violates:

> *No core is ever preempted while holding this lock, and no masked context ever waits on a holder
> that can be.*

Everything below is about how a given lock can afford to satisfy it.

### The masked micro-guard (WEDGE-7's idiom)

Mask IRQs for exactly the lifetime of the guard, at the **sole** acquisition path: mask taken before
the acquire, lock released before the unmask (in Rust that is declaration order inside the guard
struct, and getting it backwards re-opens the bug in miniature at every unlock). No holder can then
be preempted, so every critical section runs to completion once entered and every waiter — masked or
not — waits at most one critical section. Nobody has to be told `Busy`, because nobody waits long.

The precondition is that **every** section is micro-bounded: no print, no allocation, no nested
blocking lock, no I/O, no wall-clock poll. `wm::TABLE` (F1) qualifies — all 23 of its sections are
bounded `MAX_WINDOWS`-row scans, the longest being an 8-row composite snapshot with the blit itself
outside the guard — so the price is a worst-case IRQ latency of one 8-row scan. Being able to *state*
that number is the test for this idiom, and the enumeration is not optional: one unbounded section is
enough to disqualify the lock, because the mask covers all of them.

`cursor::SPRITE` (F4) is the worked negative example, and it fails on all three counts at once
(WEDGE-2's span audit): `adopt_overlay` takes `OVERLAY.lock()` — blocking, not `try_lock` — while
holding `SPRITE`, which would simply reproduce the family shape one level down; seven of its nine
sections end in a `DC CVAC` cache sweep over whole scanlines, which scales with the *panel*
(a 36-row box is ~276 KB / ~4300 cache lines on the bench's 1920x1200) rather than with the sprite;
and those sections poke up to 1296 pixels each against non-coherent scan-out — `repaint`, the F4
site, runs two such passes plus the union flush under one acquisition. Three orders of magnitude
past an 8-row scan,
and I/O besides — which the criterion excludes by name. A *partial* conversion was refused for a
reason worth repeating: hardening only the two bounded sites would leave the lock unmasked at the
seven that matter while making it look hardened.

### Claim/loan (WEDGE-8's idiom)

Put the discipline on the **lock**, not on the **work**. The lock guards a private `Option<Box<T>>`
(xHCI) or a bare availability flag (mailbox) and is held only for a masked O(1) take/put; the
resource is handed out as an RAII **loan**, and the long work runs with nothing held. Preemption
mid-hold stops being possible because there is no long hold left to preempt.

A contender is told `Busy` *immediately* rather than made to spin, and that immediacy is the second
half of the rule: **a masked caller must never wait**, since a masked waiter can take neither a
preemption nor a timer IRQ — the deadlock the model exists to prevent. Both instances therefore
check `arch::irqs_masked()` on any path that would otherwise wait. Where a caller can afford to
wait, it waits unmasked and bounded, *outside* the mask.

Two riders that both instances observe:

* **The loan is per-*transaction*, not per-function.** A path that keeps the loan while re-entering
  the module denies itself: `init_framebuffer` and `witness_fb_geometry` release before calling back
  into `mailbox` (`v3d::bringup`, `query_pitch`), just as the xHCI pumps hold only for one pass.
* **`Busy` is a new failure mode for callers**, so each instance owes a stated per-caller policy for
  it — see the table. An honest, immediate refusal is the point; a `None` from contention must never
  be indistinguishable on the wire from a `None` from dead hardware.

### The instances

Each instance makes its lock private and funnels acquisition through one or two named places, so the
invariant is checkable by grep without booting.

| lock | idiom | commit | `Busy` policy |
| --- | --- | --- | --- |
| `video::wm::TABLE` (F1) | masked micro-guard — one `TABLE.lock()`, in `fn table()` | WEDGE-7 `97357c70` | none, by construction: nobody is refused, and a waiter waits at most one bounded row scan |
| `drivers::xhci::XHCI_CONTROLLER` (F3) | claim/loan — `claim` / loan-drop / `install` | WEDGE-8 `dac6edb5` | pump/service passes skip the pass and retry ms later; the block layer surfaces `BlockError::Busy`; a masked FAT/dir RMW refuses to wait and retries the whole closure outside the `without_interrupts` span (64 attempts, `hw_wait_budget()` wall-clock), then `-EAGAIN` at the syscall boundary; diagnostics report "busy" |
| `arch::aarch64::mailbox::MBOX_FREE` | claim/loan — `claim` / `MboxLoan::drop` | MBOX-1 `c227f420` | `get_clock_rate` alone retries (`claim_bounded`, unmasked, 600 ms) because its refusal makes EMMC2 *assume* a 100 MHz SD base clock and program a wrong divider; every other entry point fails loud with `:: MAILBOX: BUSY — {op} refused … ::` and returns its documented failure value |
| `video::cursor::SPRITE` (F4) | **in flight** — claim/loan; WEDGE-7's idiom audited and refused | audit in WEDGE-2 `be4ea433` (`<D4>`, the F4 death token) | to be stated by that arc |

The F4 row is what makes the family a family rather than a coincidence of two arcs. WEDGE-2 added
`<D4>` precisely because an F4 death on `SPRITE` was reaching the wire as F1's trace and being
attributed to a lock WEDGE-7 had already closed; the span audit that came with it is the same
question this section asks of every candidate, answered "not maskable" — which names the idiom the
conversion has to use before the conversion is written.
