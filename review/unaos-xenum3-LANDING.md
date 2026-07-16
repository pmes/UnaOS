# XENUM-3 landing report — hub-downstream enumeration robustness (x86 rMBP)

Branch `hw-rmbp` (base main `e86ce35`). Lane: `unaos/crates/kernel/src/drivers/xhci/mod.rs`
+ `docs/dev/OS/07_USB_STORAGE/usb_xhci.md`. No aarch64, no syscall.rs, no files outside lane.

Both fixes are **METAL-PENDING by construction** — QEMU never posts a zeroed descriptor or a
`code 17`, so the QEMU gates prove *no regression only*.

## M1 — descriptor-content validation + short-read handling

- `sync_control` sets IOC on the DATA-stage TRB so the controller posts a data transfer event
  carrying the TRB Transfer Length residual. The sync EP0 pump claims/consumes that event
  (never reaches the async FSM) and records the actual transferred bytes in `last_control_len`.
  (First residual approach failed — the data TRB had no IOC, so no event; caught by the
  HUBSTORAGE gate rejecting a valid descriptor, then fixed with the IOC flag.)
- Bad-read predicate in the `enumerate_downstream` retry loop: BAD when the read errored,
  `last_control_len < 18`, the structural header is wrong (`bLength<18 || type!=0x01`), **or**
  the header is valid but `vid==0 && pid==0` (the exact metal strand that slipped the old
  structure-only gate). Never-valid → leave unconfigured + dispose slot.
- MPS0-learn for FS/LS behind a HS hub: read the 8-byte header first; if the device's real
  `bMaxPacketSize0` differs from the programmed guess (64 for FS), re-issue ADDRESS_DEVICE with
  the learned value (new `mps0_override` param on `address_downstream`) before the full read.

## M2 — downstream ADDRESS_DEVICE bounded paced retry

- `address_downstream` retries the same input context up to `XENUM_ADDR_RETRIES` (3) with an
  escalating settle (~200 ms × attempt) between tries, no port re-reset (a Context State Error
  is a controller-side transient, not a link fault). Root-port paced-recovery shape.
- Honest cleanup: new `dispose_downstream_slot` clears soft state + queues the deferred
  DISABLE_SLOT on the failed-address and never-valid-descriptor bail paths, so a failed
  downstream address no longer leaks an `active=true` slot with a live DCBAA pointer. Mirrors
  the root-port recovery clean-up (rings/contexts leaked-not-freed until DISABLE_SLOT lands).
  A genuinely addressed device of an unsupported class is left as-is (real device, not a failure).

## New trace substrings (for the sitting brief)

- `downstream slot N MPS0 learned M (programmed P); re-addressing.`
- `downstream slot N device-descriptor bad read (got G of 18, bLength=… type=… vid=… pid=…, attempt A of 4); retrying.`
- `downstream slot N device-descriptor never read valid after 4 attempts; leaving unconfigured.`
- `downstream ADDRESS_DEVICE code C (attempt A of N)`
- `downstream ADDRESS_DEVICE failed after N attempts`
- `downstream slot N disposed (unenumerated); queued for DISABLE_SLOT.`

## Gate results (verbatim counts)

- `./arroyo check` — `✅ x86_64 OK`, `✅ aarch64 OK` (warnings pre-existing).
- `UNAOS_HUBSTORAGE=1 ./arroyo test 60` — `>>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<`
  (×1), `downstream slot 2 MPS0 learned 8 (programmed 64); re-addressing.`,
  `>>> HUB DOWNSTREAM MASS STORAGE (slot 2, bulk in 0x81/64 out 0x2/64) <<<`, full U-arc chain
  (U5x/U7x/U8x/U9x/U10/U10c/U10d/U11x/U11m2/U6gx all PASS), 0 FAIL. No spurious bad-read/dispose.
- `UNAOS_IRQSTORAGE=1 UNAOS_FATIMG=sf ./arroyo test 200` — 25 PASS / **0 FAIL**; S-chain tags
  S3, S4, S4-mf2, S4-race, S5, S6-witness, S7-openany, S8-write, S8W, S9-grow, S9G; storage up
  (`storage_slot=1 … note='ready'`).
- `./arroyo test 40` — `>>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<`.
- `UNAOS_NOSTORAGE=1 ./arroyo test 40` — clean, 0 FAIL (U1b/U2-0a/U3/U3.5/U5x/U7x/U8x PASS).

## Honest residuals

- Neither M1's zeroed-descriptor case nor M2's `code 17` is reproducible in QEMU; both remain
  metal-pending. The MPS0-learn re-address path *is* exercised in QEMU (hubbed FS disk, MPS0=8).
- The MPS0-learn and re-address allocate fresh contexts on the second `address_downstream` call,
  leaking the first attempt's contexts (bounded, consistent with the existing leak philosophy —
  reset_soft_state does the same, and this fires only on the rare short-read path).
- Only the failed-address and never-valid-descriptor paths dispose the slot; the pre-existing
  config-descriptor-failed / no-HID paths (genuinely enumerated devices) are unchanged — out of
  M2 scope, and disposing an addressed device would be wrong.

## Files touched

- `unaos/crates/kernel/src/drivers/xhci/mod.rs`
- `docs/dev/OS/07_USB_STORAGE/usb_xhci.md` (§7d pointer + new §7g)
