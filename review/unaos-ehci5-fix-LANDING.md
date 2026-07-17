# EHCI-5-fix — LANDING (2026-07-17)

**Arc:** arm the Apple `0x44` vendor-multitouch interface (arming-order bug). Branch `r21-ehci5fix`.
**Result:** DONE. Recognizer now fires for the real metal descriptor; both regression suites green.

## Diagnosis (exact mechanism)

The 2026-07-17 rMBP 3-leg sitting captured the internal `05ac:0262` intf1 report descriptor:

```
06 00 ff 09 01 a1 03 06 00 ff 09 01 15 00 26 ff 00 85 44 75 08 96 ff 01 81 00 c0
```

Decoded: Usage Page (Vendor `0xFF00`), Usage `0x01`, Collection(Report), Report ID `0x44`,
Report Size 8, Report Count 511, **Input `81 00` = Data, ARRAY, Absolute**, End Collection.

The bug: in `parse_report_descriptor` (`ehci/mod.rs`), the vendor-page recognition sat **inside**
the field-mapping block gated on `if !is_const && is_var`. The real Apple Input is an **Array**
(`81 00`, bit1 clear), not a Variable — so `saw_vendor_input` was never set, the parser returned
`None`, and `configure_report_pointer` logged `no X/Y pointer field … not a cursor device; skipped`.
The endpoint was never armed → zero touch reports. **Not** the finger-offset hypothesis (execution
never reached the finger bytes); a recognition/arming-order gap keyed on Input *shape*.

The pre-fix self-test masked it: `vendor_multitouch_selftest` fed a **Variable** (`81 02`)
descriptor, which the `is_var`-gated recognizer accepted — so QEMU was green while metal was not.

Line anchors from the brief had drifted (tree has newer merges); verified against the real locus:
- recognizer block: was `mod.rs:1480-1483` (inside `is_var`); X/Y gate at `1506`; vendor arm at `1508`.
- skip trace at `993`; `if layout.vendor_mt` arm at `999`.

## Fix (`ehci/mod.rs` only)

1. Moved the vendor-page signature test **out** of the `is_var` block. Any **non-Constant** Input on
   `UP_VENDOR` (Array OR Variable) now sets `saw_vendor_input` / accumulates `vendor_bits` (clamped
   `count` = `report_count.min(MAX_REPORT_FIELDS)`, all `saturating_*`). The standard `has_xy` gate
   still runs first and wins, so no real pointer is diverted.
2. Extended `vendor_multitouch_selftest` to also parse the **real captured 27-byte Array descriptor**
   and assert `vendor_mt && !has_xy && report_id == 0x44`. The witness line gains
   `real-array-descriptor recognized=...`.

Bounds-safety intact: no new buffer reads; `MAX_REPORT_FIELDS` clamp, saturating advance, and every
`read_le16`/`extract_bits` bounds check unchanged. `pal.rs`/`qh.rs` untouched (report buffer did not
need to change — recognition/arming only). No protection weakened.

## Gate results

- `cd unaos && ./arroyo check` — green both arches (x86_64 + aarch64; only pre-existing warnings).
- `./arroyo test 40` — `xHCI: >>> MISSION SUCCESS <<<`, 0 FAIL. Witness:
  `vendor-multitouch self-test: recognized=true (id=0x44, min-bits=64), real-array-descriptor recognized=true, first-finger decode dx=-150 dy=5800 ok=true`.
- `UNAOS_EHCITABLET=1 ./arroyo test 40` — MISSION SUCCESS; standard-pointer path unregressed:
  `M2 armed report-pointer addr=2 ep=IN1 … (absolute; X@8/16b Y@24/16b …)`.
- Doc: `docs/dev/OS/07_USB_STORAGE/usb_xhci.md` §10c — corrected M1 wording (variable→non-Constant),
  added the EHCI-5-fix Array-vs-Variable note.

## What rides the next rMBP sitting (unchanged by this arc)

The finger DECODE offsets (`VMT_HDR_LEN`, `VMT_FINGER_ABS_X/ABS_Y/TOUCH`) remain the metal
hypothesis. This arc only proves the interface is now RECOGNIZED and ARMED (in QEMU via the real
descriptor). Metal (cursor actually moves on touch, offset confirmation) is the next attended leg.
