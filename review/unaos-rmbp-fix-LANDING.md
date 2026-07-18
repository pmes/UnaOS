# RMBP-FIX — LANDING (2026-07-18)

**Arc:** three attended-bench defects from Peter's 2026-07-18 sitting + the trackpad decode
retarget. Branch `hw-rmbp` (base = `main` `81c8433`). Lane: `drivers/ehci`, `pal.rs`, x86
serial/console init, named docs.

**Result:** **M1, M2, M3, M5 DONE** (pre-metal gate; all four QEMU gates green, 0 FAIL).
**M4 — STOP-and-report** (no code gate exists to remove; details below). The three landed fixes
are all **metal-behavior** fixes — QEMU proves non-regression, the metal verdict accrues to the
next sitting.

## M1 — bound the unbounded vendor-report dump (`ehci/mod.rs`)

`service`'s `vendor_mt` branch dumped the raw report body on report #1 + every 32nd report via
`dump_vendor_report` (a `String`-allocating serial line). Under touch that is ~100+ heap-allocating
lines/sec — the "machine appears hung" flood. The characterization it captured is complete, so it
is now bounded **hard**: `#[cfg(feature = "usbdebug")] if e.reports <= 4 { dump_vendor_report(...) }`
— usbdebug builds only, first 4 reports per device. On a default/GUI build the block **and**
`dump_vendor_report` (now `#[cfg(feature = "usbdebug")]`) are compiled out: zero dumps, zero
allocation on the hot path.

## M2 — retarget the trackpad decoder to the silicon 8-byte format (`ehci/mod.rs`)

Ground truth from the sitting (not re-litigated): the internal trackpad streams **8-byte Report ID
`0x02`** reports — `[0]`=id, `[1]`=buttons (`0x00` up/`0x01` down), `[2]`=dx i8, `[3]`=dy i8,
`[4..=5]` zero, `[6..=7]` unknown. The old `0x44`/511-byte multitouch model is **refuted** on this
path. New `decode_trackpad_rel`: length-checks (`len >= 4`), ID-gates on `0x02` (`TRACKPAD_REPORT_ID`;
short/other reports → `None`, no event, no state change), reads buttons + int8 dx/dy into the
relative `pal::Event::Mouse` seam. A bounded one-line format witness prints on the first decoded
report. `buttons` is decoded + witnessed; no click event (parity with the `is_rel_mouse` boot-mouse
path). The refuted `decode_vendor_first_finger` + `VMT_FINGER_*` + `vendor_multitouch_selftest` are
**kept as documented history** (never-trash) — the self-test still runs + passes at init. The three
now-obsolete per-endpoint `IntEp` fields (`last_x`/`last_y`/`touching`) that only served the refuted
absolute→relative conversion were removed (the decode history lives in the kept functions, not in
live-loop state).

## M3 — EHCI keyboard in `pal::pump_and_poll` (`pal.rs`)

`pump_and_poll` (the input pump a full-screen demo runs inside its own loop) serviced only
`XHCI_CONTROLLER`; the x86 `poll_input` fallback is `cfg(aarch64)`. So on the rMBP the internal
(EHCI) keyboard never posted the keystroke to exit `vug`/`pulse`. Added
`#[cfg(all(target_arch = "x86_64", feature = "ehcihid"))] crate::drivers::ehci::service_ehci_hid();`
— reusing the exact call the two main loops make (no logic duplicated). `decode_boot_keyboard`
already `push_event`s `Event::Key`, so the pump now collects them. Harmless no-op in QEMU (xHCI-only,
no EHCI HID controller armed) and compiled out on aarch64.

## M4 — GUI-build serial: STOP (no gate found to remove)

**The brief's premise — "the FTDI mirror is never brought up on the GUI path, un-gate the
bring-up" — is not supported by the code.** Investigation:

- `serial_println!` → `arch/x86_64/serial.rs::_print` → `ftdi::mirror(args)` is **unconditional**
  (from the very first boot line). `main.rs` uses `serial_println!` exclusively (62 sites, 0 plain
  `println!`), so every boot-honesty line feeds the FTDI ring in **all** builds.
- `service_ftdi()` (SET_CONFIG + the 4 FTDI vendor requests + `set_live(true)` + drain) is called
  **unconditionally** in BOTH the usbdebug loop (`main.rs:740`) and the default/GUI loop
  (`main.rs:924`). The default x86 build reaches that GUI loop (confirmed: flows past the xHCI init
  and the usbdebug block to the `console`/`Screen` setup and the `loop` at ~916).
- Repo-wide, `feature = "usbdebug"` appears only in `main.rs` and `xhci/mod.rs`, and **every** such
  site is diagnostic verbosity (health snapshots, `USB-DEBUG:` ptr/health lines) — none gate the
  FTDI transport. `ftdi.rs` has no feature gate at all. The builder's `UNAOS_USBSERIAL` is a
  QEMU-only device-attach knob, independent of the `usbdebug` kernel feature.

So in the source as it stands, a GUI build **already** brings up the FTDI console and streams the
boot lines out the cable — there is no compile/feature gate to remove. Per the CLAUDE.md STOP
tripwire (behavior diverges from the brief's stated expectations) I did **not** fabricate a
gate-removal or touch shared serial/console init on a false premise.

**The one build-conditional serial difference** is `fbcon`: usbdebug keeps fbcon attached and
mirrors serial **on-screen** (the photographable boot log); the GUI path `detach`es fbcon
(`main.rs:910`), so on GUI nothing is on-screen and serial exists **only** out the FTDI cable.
Also note the FTDI console driver is **xHCI-only** — a dongle enumerated on the EHCI companion has
no console bring-up in any build.

**Recommendation for the bench / integrator:** confirm whether the "zero serial" GUI boot had the
FTDI dongle on an **xHCI-routed** port (that, not a build gate, decides whether GUI cable serial
appears), or whether "serial" at that sitting meant the on-screen fbcon mirror (which GUI detaches).
If the intent is on-screen boot-honesty lines on GUI, that is an fbcon-detach policy change, not an
FTDI-transport un-gate — please confirm scope before I touch it.

## Gate results (verbatim)

- `cd unaos && ./arroyo check` — `✅ x86_64 OK` and `✅ aarch64 OK`. No new warnings from the change
  (the two `ehci/mod.rs` warnings at `:618` `live_port_smoke` and `:1982` `mut cg_cleared` are
  pre-existing and untouched).
- `./arroyo test 22` — `xHCI: >>> MISSION SUCCESS (BOT + CSW). TARGET ACQUIRED. <<<`, **0 FAIL**.
  Witnesses: `report-parser self-test: … bounded=true … legit X/Y parse ok=true`;
  `vendor-multitouch self-test: recognized=true (id=0x44 …) … ok=true` (refuted-hypothesis self-test
  kept + passing); `[0] M2 armed keyboard addr=1 ep=IN1 … (boot protocol)`.
- `UNAOS_CPU=qemu64 ./arroyo test 22` — MISSION SUCCESS, **0 FAIL**.
- `UNAOS_EHCITABLET=1 ./arroyo test 22` — MISSION SUCCESS, **0 FAIL**; `[0] M2 armed report-pointer
  addr=2 ep=IN1 … (absolute; X@8/16b Y@24/16b …)` — the standard `usb-tablet` path arms unchanged
  (the retarget touches only the `vendor_mt` branch).
- `./arroyo test-arm 22` — **0 FAIL / 0 PANIC**; HID enumerates (`MOUSE-1: HID pointer detected …`).
  aarch64 `pump_and_poll` is byte-identical (the added EHCI call is x86-only cfg).

## QEMU-vs-metal honesty

All three landed fixes are **metal-behavior** fixes on the real `05ac:0262` trackpad / internal
keyboard. QEMU's `usb-tablet` never sets `vendor_mt` and QEMU has no EHCI HID, so these gates prove
**non-regression only**. The metal verdict — the retargeted 8-byte decode moves the cursor from the
trackpad, the internal keyboard exits a full-screen demo, and touch no longer floods the console —
accrues to the next attended sitting.

## Docs (M5)

`docs/dev/OS/07_USB_STORAGE/usb_xhci.md` §10e added (dump bound + 8-byte `0x02` decode of record +
the EHCI-keyboard pump). This landing report. (Serial/console bring-up doc unchanged — M4 landed no
serial-transport code change; see the M4 STOP note.)
