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
