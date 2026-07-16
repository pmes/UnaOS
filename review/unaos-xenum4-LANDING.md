# XENUM-4 landing report — Evaluate Context MPS0 fix (x86 rMBP track, R18)

## Summary
Replaced the XENUM-3 hub-downstream MPS0-learn **re-ADDRESS** apply (refused by real Panther
Point silicon with completion code 19, Context State Error) with an **Evaluate Context**
command (TRB type 13, xHCI 4.6.7) — the standard mechanism for correcting EP0 Max Packet Size
on a slot already in the Addressed state. The learn step (8-byte-header read + predicate) is
unchanged; only the apply mechanism changed.

Changes (lane: `unaos/crates/kernel/src/drivers/xhci/mod.rs` + `docs/dev/OS/07_USB_STORAGE/usb_xhci.md`):
- New `evaluate_downstream_ep0_mps(slot_id, mps0)`: input context A1-only (A0 clear per 4.6.7),
  EP0 context copied from the live output/device context (preserves EP Type / CErr / TR Dequeue
  Pointer), MPS0 patched, issued via `run_command_sync`. Reuses the slot's existing output
  context, EP0 ring, DCBAA pointer and slot state — no fresh allocations, no DCBAA rewrite, no
  second ADDRESS_DEVICE.
- `enumerate_downstream`: the MPS0-learn branch now calls `evaluate_downstream_ep0_mps` on success
  continuing straight into the 18-byte descriptor read; on failure it traces the code and falls
  through to the existing `dispose_downstream_slot` bail (no retry storm).
- Removed the dead re-address plumbing: `address_downstream` no longer takes an `mps0_override`
  parameter; its EP0 MPS0 guess is now the plain speed-based value. Single MPS0-application path.
  The XENUM-3 first-attempt context-leak residual (a re-address allocated a second context set)
  disappears with the re-address — Evaluate Context reuses the existing contexts.
- Doc: usb_xhci.md §7g-4 subsection (what/why/plumbing-removed/trace substrings/gates/metal-pending).

## Gate results (verbatim)

`./arroyo check` (both arches):
```
✅ x86_64 OK
✅ aarch64 OK
```

`UNAOS_HUBSTORAGE=1 ./arroyo test 60`:
```
xHCI: downstream slot 2 MPS0 learned 8 (programmed 64); Evaluate Context.
xHCI: downstream slot 2 EP0 MPS updated via Evaluate Context (8).
xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<
FAIL count: 0   PASS count: 17
```
(NO `re-addressing` line present; the hubbed FS disk enumerates through the new path.)

`UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf ./arroyo test 200`:
```
✅ Test run complete.
FAIL count: 0
```

`./arroyo test 40`:
```
MISSION: 1  FAIL: 0
```

`UNAOS_NOSTORAGE=1 ./arroyo test 40`:
```
FAIL: 0  (clean)
```

## Trace substrings (bench-assertable)
- `downstream slot N MPS0 learned M (programmed P); Evaluate Context.`
- `downstream slot N EP0 MPS updated via Evaluate Context (M).`
- `downstream slot N Evaluate Context code C; disposing.` (failure form)
- The XENUM-3 `re-addressing.` line no longer appears.

## Honest residuals
- **METAL-PENDING by construction.** QEMU accepts both the old re-address and the new Evaluate
  Context, so the HUBSTORAGE gate proves the new path *works*, not that it *cures* the code-19
  metal strand. The rMBP sitting must assert the FS mouse behind the HS hub now enumerates and
  tracks (descriptor read completes at the learned MPS0, HID interrupt endpoint bound).
- The Evaluate-Context-failure trace (`code C; disposing.`) never fires in QEMU (the command
  succeeds) — that path is exercise-pending on silicon, same as the old code-19 wall.
- Pre-existing unrelated warnings remain (unnecessary-unsafe at mod.rs:5696/5721, dead
  RING_SIZE in ring.rs) — untouched, outside this arc's scope.
