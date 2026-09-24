# USBNET2 — prep

## The finding

Flight 15 §2 "The dongle" (`docs/dev/evidence/rmbp-0915/flight15/FLIGHT15.md:27`):
`[1297ms] :: EHCI-HID: [1] M1 hub-downstream device addr=3 0b95:1790 class=0x00 speed=HS
depth=1 (parent hub 1 port 2)` — the AX88179 enumerated on the **EHCI** [1] hub, not the
xHCI. USBNET (SO56) is an xHCI-path driver and `UNAOS_USBNET=1` was not armed. Flight
doc's words: "the port Peter used is behind the EHCI hub, so USBNET needs an EHCI
front-end (the `ftdirx`/`ehcihid` pattern) or Peter uses the other port. DECISION for
Peter: which port did he use, and try the other one next flight with the knob armed."

## Mechanism

- **USBNET today is xHCI-only.** `xhci/usbnet.rs:17-70` (module doc), `98-109`
  (KIND_AX88179): the AX88179 vendor bring-up (register I/O over vendor requests, RX
  trailer parse, TX header) is fully written but claimed only from the xHCI event-ring
  dispatch (`xhci/mod.rs:4461`, `17220 service_usbnet`). `e1000.rs:1039,1047,1053` fall
  back to `xhci::usbnet::raw_rx/raw_tx/hw_addr` — no EHCI equivalent hook exists.
- **Port routing already exists and is default-ON: PORTSW-1**
  (`docs/dev/OS/07_USB_STORAGE/usb_xhci.md:1900-1990`). Four PCI config regs on the xHCI
  function: `0xD0 XUSB2PR` (route select, 0=EHCI/1=xHCI), `0xD4 USB2PRM` (mask), `0xD8
  USB3_PSSEN`, `0xDC USB3PRM`. Runs in `pci::init` before `xhci::init`, mask-read-before-
  write (`SELECT |= mask`, never forces an unmasked bit). Metal verdict (same doc,
  2026-07-16): on this rMBP unit, mask reads 0xf and the flip routes 0xf->0xf — every
  switchable USB2 root port. **Open question this raises**: is the AX88179's parent hub
  (EHCI hub 1, port 2, depth=1) on one of those four switchable root ports, or outside the
  mask? PORTSW-1 moves a whole root port's downstream tree; it can't help if this hub's
  uplink isn't in the mask.
- **CORRECTION to the brief's grep target**: `ftdirx` is not in `ehci/mod.rs` — it's in
  `xhci/ftdi.rs` (xHCI-side). The real EHCI-side template for a non-HID bulk device is the
  **Bluetooth ACL path** in `ehci/mod.rs` (feature `bt`): `bt_acl_txn` (`:7800-7913`)
  builds one QH, sets `QH_DTC` (software owns the toggle, carried in/out as a return
  value), sets TT hub-addr/hub-port for a device behind a hub (exactly hub 1 port 2's
  shape), links onto `self.async_qh`, flips `USBCMD.ASE` on, polls the overlay's
  `QTD_ACTIVE` under a TSC budget, flips `ASE` off and waits for `USBSTS.ASS` to clear
  (`:7871-7887` — must be observed, not just requested, or a late DMA corrupts the shared
  EP0 buffer). `bt_acl_tog_set` (`:7917-7935`) is the toggle bookkeeping; `bt_l4_att`
  (`:7953+`) is the caller shape: one "why unreachable" line before anything is sent.
- Ledger: `rmbp-ledger.md` B226 (this ARC), `LEDGER.md` SO56 (USBNET "fixed-unflown" — the
  AX88179 front-end is built but has never claimed a device, because the device never
  reached xHCI on the boot that had it plugged in).

## Plan

**M1 — read-only port-route witness (no chipset write).**
Files: `unaos/crates/kernel/src/arch/x86_64/pci.rs` (PORTSW-1's site — read it first to
confirm the exact fn/offsets). Add one line, on the *existing* default-on path, printed
before the mask-guarded write (the genuine cold value this boot):
`:: PORTSW-1: portroute xusb2pr={:#06x} pssen={:#06x} mask={:#06x} ::`.
Go-red fixture: stub the PCI config read to return mask `0x0` — the line must read
`mask=0x0000` and the existing write-skip path must still fire; red = mask nonzero but
line absent.

**M2 — which root port the AX88179's hub uplink sits on.**
Same file, cross-referencing the root-port index PORTSW-1 iterates against
`drivers/ehci/mod.rs`'s hub-walk fields (`hub_addr`/`hub_port`, `:130-138`). Print
`:: PORTSW-1: rootport idx=N routed=xhci|ehci ::` per switchable bit so a boot with the
dongle plugged in says, in one place, whether hub 1's uplink moved — the fact Peter's
"try the other port" call needs before a bench cycle.
Go-red fixture: same mask=0 stub — every `idx` line must read `routed=ehci`; red = any
`idx` line reading `routed=xhci` under a 0 mask.

**M3 — EHCI-side AX88179 front-end** (only if M2 shows the hub's port is NOT switchable,
or Peter says "use this port"). Follows `bt_acl_txn`'s shape, not `ftdirx`'s (correction
above). New `drivers/ehci/usbnet_ehci.rs` (or a `#[cfg(feature = "usbnet_ehci")]` section
in `ehci/mod.rs` beside `btbond.rs`):
  - reuse `usbnet::ax::*` register-level AX88179 bring-up from `xhci/usbnet.rs` (bus-
    agnostic part logic) behind a transport trait instead of raw xHCI TRB calls — that
    refactor is the real size of this rung.
  - bulk-OUT TX: one `bt_acl_txn`-shaped call per frame, same toggle discipline.
  - bulk-IN RX: **not** a one-shot like BT-L4 — needs continuous re-arm each
    device-service pass, closer to `service_ehci_hid`'s per-frame poll. New: no existing
    EHCI code re-arms a bulk-IN across passes.
  - hub_addr/hub_port for the QH come from the hub-walk's recorded `(hub 1, port 2)`
    (already captured — the flight's own enumeration line proves the walk knows this).
Witness: `:: USBNET-EHCI: kind=ax88179 hub=1 port=2 mac=xx:xx:xx:xx:xx:xx -> PASS|FAIL ::`,
per-pass `:: USBNET-EHCI: rx=N tx=N rx_drop=N tx_drop=N errors=N ::` (mirrors
`usbnet.rs:650`).
Go-red fixture: drop the toggle-carry so every other bulk-IN uses DATA0 — the peer
discards alternating packets; assert `rx` grows monotonically pass-over-pass.

**Recommendation**: M1+M2 first (cheap, read-only, answers Peter's own question — is his
port switchable) before committing to M3's refactor. If M2 shows the hub's port IS in the
switchable mask but still reads `routed=ehci`, that is a PORTSW-1 *ordering or coverage*
bug (cheaper fix than a whole EHCI front-end) and should be chased before M3.

## Spec pins

For `unaos/scripts/specs/rmbp-boot.spec` (or a new `usbnet-ehci.spec` once M3 lands):
```
REQUIRE :: PORTSW-1: portroute xusb2pr=[0-9a-fx]+ pssen=[0-9a-fx]+ mask=[0-9a-fx]+ ::
REQUIRE :: PORTSW-1: rootport idx=\d+ routed=(xhci|ehci) ::
```
(the mask=0 "no idx reads xhci" pin has no line-conditional FORBID in this spec engine —
it is asserted by the go-red fixture, not the static spec.) Once M3 exists:
```
REQUIRE :: USBNET-EHCI: kind=ax88179 hub=1 port=2 mac=[0-9a-f:]{17} -> PASS ::
FORBID :: USBNET-EHCI: .* -> FAIL ::
FORBID :: USBNET: up kind=ax88179  (would mean it wrongly claimed the xHCI path instead)
```
No regex look-around anywhere above (`spec` engine constraint) — all pins are plain
anchored substrings/character classes.

## Open questions

- Which physical port did Peter plug the dongle into for flight 15 (needed to correlate
  with M2's per-port `routed=` line against the physical layout)?
- Does Peter want M3 attempted at all, or just "the other port" so PORTSW-1 (already
  default-on) carries the dongle to xHCI where USBNET already works? If the other port is
  switchable, M3 is unnecessary.
- R68 (`docs/dev/RULINGS.md:99`, same day/session) is Peter's go to "write whatever is
  needed to the chipset to enable functionality," given for the IOAPIC/PIRQ rung, and says
  "the go stands for any rung that later needs one." Does that cover a *new* XUSB2PR-
  adjacent write if M2 finds a routing gap PORTSW-1 doesn't already close, or does Peter
  want a fresh yes? (PORTSW-1's own existing write is already covered by its prior fold.)

## Next-session start

1. `sed -n '1,60p' unaos/crates/kernel/src/arch/x86_64/pci.rs | grep -n "XUSB2PR\|USB2PRM\|USB3_PSSEN\|USB3PRM\|fn "` to find PORTSW-1's exact function and offsets before touching anything.
2. `grep -n "PORTSW-1" unaos/crates/kernel/src/arch/x86_64/pci.rs` to find the write site and add the M1 pre-write witness line right above it.
3. Re-read `docs/dev/evidence/rmbp-0915/flight15/FLIGHT15.md` in full (only §2 was read this round) for any other port/topology detail before adding M2's per-rootport line.

## Draft code (unbuilt)

```rust
// unaos/crates/kernel/src/arch/x86_64/pci.rs — anchor: immediately before PORTSW-1's
// existing mask-guarded write (find via `grep -n "XUSB2PR" pci.rs`; not read this round,
// so this is illustrative of shape, not a diff).
serial_println!(
    ":: PORTSW-1: portroute xusb2pr={:#06x} pssen={:#06x} mask={:#06x} ::",
    xusb2pr_before, pssen_before, usb2prm_mask
);
```
