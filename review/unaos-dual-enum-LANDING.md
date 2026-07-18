# DUAL-ENUM — adjudication: the both-stacks FTDI observation (R22, hw-rmbp)

**Verdict: BENIGN temporal double-enumeration. No code fix. Does NOT explain the GUI-serial silence.**

Source-read arc (hw-rmbp). Worktree `../UnaOS-rmbp` @ `33fb54b`, level with `main`. No commits.

## The observation (2026-07-18 sitting, non-relitigable)

On a usbdebug boot the FTDI dongle (`0403:6001`, a Full-Speed device) was logged by BOTH stacks
in one boot: xHCI claimed it (slot 2, port 2, console UP, 58087 bytes) AND the EHCI walk logged
`M1 hub-downstream device addr=2 0403:6001 class=0x00 speed=FS (hub 1 port 2)`. On Panther Point
XUSB2PR muxes each USB2 port to exactly one controller, so a port routed to xHCI "should be"
invisible to EHCI. This document adjudicates that against source + the boot log of record
(`~/unaos-bench/rmbp-serial-2026-07-18-r22-bootA.log`).

## Timeline of record (who enumerates what, when, relative to the mux write)

The x86 PCI-init path is strictly sequential in one function (`arch/x86_64/pci.rs::init`), the same
call order in EVERY build (usbdebug adds serial logging only — it cannot reorder these calls):

1. **`ehci::init()`** (`pci.rs:222`, feature `ehcihid`, DEFAULT-ON) — runs to completion FIRST,
   while XUSB2PR is still at the Apple-EFI default `0x0` (all USB2 ports on EHCI). It scans every
   EHCI function, wakes it, walks its root ports, and recursively enumerates the RMH tier
   (`ehci/mod.rs::init` → `reset_root_port` → `enumerate_at_zero` → `bring_up_hub`). This is where
   the FTDI `0403:6001` is enumerated (SET_ADDRESS, GET_DESCRIPTOR) as a downstream child of the
   RMH. **This is a blocking call; it fully returns before step 2.**
2. **`enable_intel_xhci_ports()`** (`pci.rs:255`) — the mux flip: `XUSB2PR mask=0xf routed 0x0->0xf`.
   Port 2 (and the other switchable USB2 ports) now route to xHCI.
3. **`xhci::init()`** (`pci.rs:258`) — the controller is reset+started, generates a connect event
   on root port 2, resets the port (returning the device to address 0), addresses+configures the
   FTDI on the xHCI bus, and arms the FTDI console.

The boot log confirms this order exactly — every `:: EHCI-HID: ... ==witness ::` line (including the
FTDI's) precedes `:: PORTSW-1: XUSB2PR ... 0x0->0xf ::`, which precedes `xHCI: >>> FTDI ... DETECTED`.

**Answer to Q1:** There is NO concurrent claim. The two enumerations are separated in time by the
mux flip. EHCI walks the port while it is EHCI-routed (XUSB2PR=0x0); xHCI walks it after the flip
(XUSB2PR=0xf). The "exactly one controller" invariant holds at **every instant** — the device was
simply enumerated twice, once on each side of the flip. Benign-but-confusing, exactly the first horn
of the brief's Q1, and definitively NOT the second (two drivers issuing transfers concurrently).

## Port map (Q2) — provably the same single device

- **EHCI side:** `hub 1 port 2` = the Intel Rate-Matching Hub (`8087:0024`, class 0x09, enumerated as
  hub addr 1 on EHCI controller **[1]**, which advertises 8 downstream ports) — downstream port 2.
  The RMH is how a HS-only EHCI reaches FS/LS devices on the shared USB2 ports.
- **xHCI side:** root port 2, slot 2. xHCI handles FS natively (no RMH), so the same physical
  receptacle presents as a direct root-port device.
- **Same device?** Yes, provably. One FTDI dongle was plugged; `0403:6001` appears exactly once on
  each side, on opposite sides of the flip. The matching "port 2" is partly coincidental (RMH
  downstream-port numbering vs xHCI root-port numbering are different domains), but the device
  identity (the single `0403:6001`) is unambiguous. Not two different devices.
- Note the same log shows the **internal trackpad** (`05ac:0262` bcm5974) and the Broadcom BT
  (`0a5c:4500`) reaching EHCI through the SAME RMH (hub 1 → SMSC hub `0424:2512` addr 3 → its
  children). The external FTDI and the internal HID share the RMH on the EHCI side — this matters for
  the fix analysis below.

## Does the EHCI claim harm the xHCI console? (Q3) — No.

- **The FTDI arms nothing on EHCI.** It is class 0x00 with no HID interrupt-IN endpoint, so
  `configure_hid` logs "no HID interrupt-IN endpoint — nothing to arm" and adds nothing to
  `int_eps`. `service_ehci_hid` → `Controller::service` iterates ONLY `int_eps`; there is no port
  re-walk, no rescan, no hot-plug loop in this arc. **After `init()` returns, EHCI never touches the
  FTDI again.** It therefore physically cannot issue a late SET_ADDRESS/SET_CONFIG that resets the
  FTDI's endpoints mid-stream. The "EHCI claim lands AFTER xHCI console bring-up and silences it"
  hypothesis is refuted twice over: by source (sequential init — EHCI fully returns before the flip
  and before xHCI starts) and by the log (all EHCI lines precede the flip).
- **The metal evidence is decisive.** The log of record contains two boot passes:
  - Pass with the FTDI ON EHCI (`addr=2 0403:6001 ... hub 1 port 2`): xHCI console UP, 58087 bytes.
  - Pass where EHCI's FTDI SET_ADDRESS FAILED (`address 2 BURNED`): xHCI console UP, 24365 bytes.
  Both boots brought the console up. The console's health is **independent** of whether EHCI touched
  the FTDI — and the boot that exhibits the full double-enum is precisely a boot where serial WORKED.
- **Build ordering cannot differ.** The `ehci::init → flip → xhci::init` order is unconditional and
  build-independent; usbdebug only slows things uniformly. No build can make EHCI's one-shot claim
  land after the xHCI console arms, because EHCI's claim happens before the flip in all builds.

## GUI-silence adjudication (confidence: HIGH that dual-enum is NOT the cause)

The dual-enum is **eliminated** as the GUI-zero-serial cause. It is present in the WORKING
(console-UP) boots, it is a one-shot pre-flip event that leaves no live EHCI transfer, and its
ordering is build-invariant. Whatever silences serial on GUI (non-usbdebug) builds, it is not the
EHCI stack re-claiming the FTDI. That remains a **separate open bench question** (already flagged in
`unaos-metal-rmbp.md` as owed bench evidence, NOT code) and is out of this arc's scope.

## Why no fix ships

The brief's candidate — "EHCI skips ports XUSB2PR routes away, checked at walk time" — is both
unnecessary and unsafe here:
1. **Unnecessary:** benign; the double-enum coexists with a healthy xHCI console (proven above).
2. **Undecidable at walk time:** at EHCI-init the flip has not happened (XUSB2PR=0x0), so EHCI cannot
   yet know which ports will move. USB2PRM's switchable-port mask describes the shared EHCI/xHCI
   ROOT ports, not RMH-**downstream** ports — and the FTDI reached EHCI as an RMH-downstream child,
   not on a bare EHCI root port. There is no clean walk-time predicate mapping "RMH downstream port
   N" to "will be routed to xHCI."
3. **Risk to a working path:** the internal trackpad (metal-proven on EHCI) hangs off the SAME RMH as
   the external FTDI. Any heuristic that skips RMH subtrees or "switchable" ports risks dropping the
   trackpad. This is exactly the STOP condition — touches a shared/uncertain seam and could weaken a
   working protection/path — so per the brief: **do not code, write the adjudication.**

Minor, out-of-lane doc nit (flagged, not changed): the `ehci/mod.rs` module header claims "the two
stacks own disjoint ports by hardware." That is true only AFTER the flip; at EHCI-init time
(pre-flip) EHCI transiently sees switchable-port / external devices too (the FTDI is the proof). The
statement is a benign over-simplification of the steady state, not a functional bug.

## Bench asks (to settle the one remaining fork — the GUI silence, a separate arc)

The dual-enum forks (Q1–Q3) are settled by the existing log; no new bench data is needed for them.
For the GUI-serial silence (NOT this arc, but the adjacent open question the dual-enum was floated to
explain):

1. **A secondary witness channel for the GUI build.** Serial is the thing that's zero, so it cannot
   witness itself. Instrument the GUI-build framebuffer (or an on-screen counter) to report whether
   the PORTSW flip line and the `FTDI console up` bring-up were reached — i.e. distinguish "console
   never armed" from "console armed but TX never leaves the wire."
2. **External confirmation the FTDI enumerated on xHCI in the GUI build** — e.g. the dongle's own
   status, or a second host observing the FTDI's USB enumeration, to localize the silence to
   pre-enumeration vs post-arm-no-output.

These would localize the GUI silence; none bears on the dual-enum verdict, which is closed here.

## Gate

No code shipped → no gate beyond this write-up (per the brief).
