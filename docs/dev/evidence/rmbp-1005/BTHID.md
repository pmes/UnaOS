# BTHID — a Bluetooth HID host on the rMBP (rmbp-ledger B339)

Branch `exec-rmbp-bthid`, cut from 53423b84 (boot 21's image). Written from the Bluetooth Core
specification (Vol 4 Part E HCI, Vol 3 Part A L2CAP, Vol 3 Part B SDP, Vol 3 Part C §5 Simple Secure
Pairing) and the HID-over-Bluetooth profile (HIDP 1.1: PSM 0x11 control, PSM 0x13 interrupt, the
0xA1 DATA|Input header, SET_PROTOCOL, the SDP HIDDescriptorList attribute 0x0206). No BlueZ/Linux.

## Finding

Boot 20 (`f20-boots.log`, `bt-l0`/`bt-l1`/`bt-c1`) shows the Broadcom radio (manufacturer 0x000f,
HCI 4.0, on EHCI behind the internal hub) answering every command with status 0x00, running a full
10240 ms GIAC inquiry (`responses=0`) and two blind 5120 ms page trains at Peter's MEGABOOM — the
existing campaign is an experiment aimed at one speaker, run once at boot inside `service_ehci_hid`
under the `EHCI_HID` lock (21.8 s in which the EHCI keyboard/trackpad poll does not run). There is
no generic inquiry table, no pairing a user can drive, no HID channel, no report routing, and no
reconnect. The A2DP-only L2CAP (`bt_c2_*`) and the RAM-only SSP (`bt_ssp_pair`) are speaker-shaped
and synchronous.

## The seam

* **Kernel — driver.** The radio is hardware on the EHCI controller; the code is a child module
  `drivers/ehci/bthid.rs` reusing the parent's transport primitives (`bt_hci_send`,
  `bt_read_full_event`, `bt_acl_txn`, the event-endpoint slot) and the parent's HID machinery: the
  pointer field map comes from the SAME `parse_report_descriptor` the EHCI-HID USB path uses, the
  report decode from the same `decode_report_pointer`/`decode_boot_keyboard`, and every event goes
  into the same `pal::push_pointer_report` / `pal::push_event` ring. No second HID decoder.
* **Never holding input.** After a short synchronous bring-up (≤ 11 bounded commands) the stack is a
  state machine stepped once per `service_ehci_hid` pass: one HCI command in flight, events read with
  a zero first-packet budget (an idle endpoint costs one token read), ACL polled at most every 2 ms
  with a 250 µs budget. The pass takes the BTHID state with `try_lock` and skips when a shell verb
  holds it. Every host-side wait has a deadline.
* **The link key store is an attribute.** One object per bonded device at
  `<home>/.config/unaos/bt/<addr12>` carrying typed attributes `bt.linkkey` (Blob, 16),
  `bt.keytype` (Int), `bt.name` (Str), `bt.class` (Int) and, when it fits, `bt.hiddesc` (Blob, the
  report descriptor). Written and read OUTSIDE the `EHCI_HID` lock from the storage-ready passes in
  `main.rs` (the holocron rule). On a FAT root the attribute surface answers `-ENOTSUP`; that is
  witnessed and the key lives in RAM for the session. Holocron (CODEX identity/secrets) owns this
  charter later; until then the object is the store — the BT-BOND holocron file is not written.
* **Knob.** `btc` (BR/EDR, implies `bt`). Under `btc` the boot campaign no longer runs the legacy
  LE-scan/inquiry/page chain at boot; it runs the BTHID bring-up (and, once the bond store loads after
  the desktop, the bounded 5 s reconnect). The legacy chain is still reachable by Ctrl+Alt+B. A
  `bt`-only image keeps the LE campaign and the `bt` verb says BR/EDR is not built.

## Milestones

* **M1 INQUIRY** — `bt scan`: HCI_Inquiry (GIAC, 10.24 s, host deadline 11 s then Inquiry_Cancel),
  Inquiry Result / with RSSI / Extended (EIR name), then one Remote_Name_Request at a time (host
  deadline 6 s each, cancel on expiry); `bt` prints the table (address, class decoded, RSSI, name).
* **M2 PAIRING** — `bt pair <addr|#n> [yes|no]`: Create_Connection, Authentication_Requested; Link
  Key Request answered from the store (negative when pairing anew); IO capability DisplayYesNo,
  general bonding with MITM; NoInputNoOutput peer = just works (auto-confirm); DisplayYesNo peer =
  numeric comparison on the glass, confirmed by `bt pair <addr> yes` (60 s, then negative); a
  keyboard peer = passkey notification shown for typing; legacy PIN request answered `0000`;
  Link Key Notification staged into the store; Set_Connection_Encryption on.
* **M3 L2CAP** — signalling (connection request/response both directions with a pending answer
  until encryption, configuration with MTU 672 both directions, disconnection, echo, information
  "not supported"), channels SDP 0x0001, HID control 0x0011, HID interrupt 0x0013; ACL credits from
  Number Of Completed Packets.
* **M4 HID** — the report descriptor by SDP (ServiceSearchAttributeRequest, UUID 0x1124, attribute
  0x0206, continuation up to 8 rounds); pointer layout by the shared parser (per Report ID, by
  feeding it the descriptor prefix that ends at the next Report ID item), keyboard and wheel fields
  by a small classifier walk; fallback SET_PROTOCOL(boot) and the HIDP boot reports (ID 1 keyboard,
  ID 2 mouse) when no descriptor comes; `bt connect <addr>`; inbound connection requests from a
  bonded device accepted; reconnect at desktop-ready (gui stamp + store loaded), paging bonds one at
  a time inside one 5 s deadline.
* **M5** — `tests bt` → `:: BTHID: inquiry=<n> paired=<n> connected=<n> hid_reports=<n> -> PASS|SKIP ::`
  over five radio-free fixtures (inquiry-result parse, EIR name, SDP descriptor extraction,
  descriptor classification through the shared parser, L2CAP configure option parse).

## Witness

`:: BTHID: up bd_addr=… acl=<len>x<num> ssp=on page_scan=on at=<ms> ::` on every `btc` boot with a
radio; `:: BTHID: store loaded bonds=<n> …` after the session's storage pass; with bonds,
`:: BTHID: reconnect start bonds=<n> deadline=5000ms (gui at <ms>) ::` and a `reconnect … ->` line per
bond. Every HCI command `:: bthid: HCI <name> (<opcode>) status=<0x..> ::`.

## Owed

Holocron as the key's owner (the attribute object is the interim store; its read gate is the
object's); the parse of keyboard and wheel inside the shared parser (today a classifier beside it);
Apple Magic Mouse multitouch mode (its default report is decoded as a plain mouse); LE HID (HOGP);
a passkey ENTRY path (we display, never type); persisting the device table between boots.
