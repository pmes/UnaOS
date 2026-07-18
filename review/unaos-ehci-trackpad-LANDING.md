# EHCI-TRACKPAD — LANDING (2026-07-18)

**Arc:** the bcm5974 vendor mode switch — a class feature-report handshake that starts the
internal trackpad's multitouch stream. Branch `hw-rmbp`.
**Result:** DONE (pre-metal gate). The Apple vendor-multitouch interface is now RECOGNIZED,
ARMED (EHCI-5), and MODE-SWITCHED (this arc). Both QEMU regressions green; metal verification
(cursor moves from the internal trackpad) is deferred to the next attended sitting.

## The gap this closes

EHCI-5(-fix) proved the internal trackpad's vendor-multitouch interface (`05ac:0262` intf1,
Report ID `0x44`, usage page `0xFF00`) is recognized and its interrupt endpoint armed. But on
metal the stream never starts: the trackpad ships in a **single-touch compatibility mode** and
emits **no** `0x44` frames until told to. The Linux `bcm5974` driver flips it out of that mode
with a class feature-report handshake (`bcm5974_wellspring_mode`). This arc ports that handshake.

## M1 — `bcm5974_mode_switch` (`ehci/mod.rs`)

Fired **once, before arming**, inside `configure_report_pointer` whenever `layout.vendor_mt` is
recognized. Three EP0 control transfers via the existing overlay-direct / chain-mode `control()`:

1. **GET_REPORT(Feature):** `bmRequestType 0xA1` (IN|CLASS|INTERFACE), `bRequest 0x01`,
   `wValue 0x0300` (report type 3 Feature, id 0), `wIndex 0x0000`, `wLength 8` → into `data_buf`.
2. **Flip byte 0:** `data_buf[0] = 0x01` (`BCM5974_MODE_VENDOR`, raw multitouch; `0x08` = NORMAL).
   Remaining 7 bytes echoed back as read (on a failed read, the buffer is zeroed then byte 0 set).
3. **SET_REPORT(Feature):** `bmRequestType 0x21` (OUT|CLASS|INTERFACE), `bRequest 0x09`,
   `wValue 0x0300`, `wIndex 0x0000`, `wLength 8` → writes the modified report back.

All constants (`BCM5974_MODE_*`) are the Linux driver's **verbatim**. One carried nuance:
`wIndex` is the driver's `REQUEST_INDEX` (**0**), NOT the interface number — the value proven on
real MacBooks. The interface number is logged alongside so a sitting can retry with `intf` if
index 0 STALLs on this exact `0262`. Every stage logs status; **any stall/timeout is non-fatal**
(a firmware already streaming needs no switch), so a failed handshake never un-arms the endpoint.

## M2 — decode (unchanged from EHCI-5)

The first-finger decode into the relative `pal::Event::Mouse` path (the XENUM FS-mouse seam)
already landed in EHCI-5 (`decode_vendor_first_finger`, `service` at the `l.vendor_mt` branch).
The switch only makes the frames arrive; the `VMT_FINGER_*` offsets remain the metal hypothesis.
No decode changes were needed and none were made (arc discipline — not gold-plated).

## QEMU unchanged BY CONSTRUCTION

The switch is gated on `vendor_mt`, which QEMU's `usb-tablet` (standard absolute pointer) never
sets, so `bcm5974_mode_switch` is never reached in QEMU. Verified: both regressions MISSION
SUCCESS with **zero** `bcm5974 GET_REPORT` / `SET_REPORT` lines; keyboard and tablet paths arm
exactly as before. This satisfies the overlay-direct vs chain-mode duality hard law — the metal
path (overlay-direct control transfers) is exercised only on the real device; QEMU's chain-mode
path is untouched because it never enters the switch.

## Gate results (verbatim)

- `cd unaos && ./arroyo check` — `✅ x86_64 OK` and `✅ aarch64 OK` (only pre-existing warnings;
  no new warnings from the added consts/method — all are used).
- `./arroyo test 22` — `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<`, 0 FAIL.
  Witnesses: `vendor-multitouch self-test: recognized=true (id=0x44 …) … ok=true`;
  `[0] M2 armed keyboard addr=1 ep=IN1 … (boot protocol)`. **No** `bcm5974 GET/SET_REPORT` line.
- `UNAOS_EHCITABLET=1 ./arroyo test 22` — MISSION SUCCESS; `[0] M2 armed report-pointer addr=2
  ep=IN1 … (absolute; X@8/16b Y@24/16b …)` — the standard usb-tablet path unchanged; no switch.
- Keyboard path provably untouched: `M2 armed keyboard` identical to pre-arc; the switch code is
  entered only on `vendor_mt` (proto 0, vendor page) — the keyboard is a boot interface (proto 1).
- Doc: `docs/dev/OS/07_USB_STORAGE/usb_xhci.md` new §10d (the bcm5974 mode switch, constants,
  wIndex nuance, QEMU-unchanged proof, metal trace list).

## What rides the next rMBP sitting

- The switch actually starting the stream: on the real `0262`, the `M1 bcm5974 SET_REPORT(feature)
  … mode=0x01 — multitouch stream requested` line, then the §10c `vendor-multitouch raw report`
  dumps beginning **on touch**. If `SET_REPORT` FAILED prints, try `wIndex = intf`.
- Cursor motion from the trackpad + the `VMT_FINGER_*` offset confirmation (the EHCI-5 hypothesis).
- The `Buf64` 64-byte read ceiling (§10c) if the metal capture shows finger fields beyond 64 B.
