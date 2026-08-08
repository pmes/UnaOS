# PREDICTION — bt-l0b (GR22)

Written before the next BT boot. Everything below is falsifiable from one capture
slice, read with `awk '/bt-l0/' <log>` (never `grep`).

Target: rMBP 2012, EHCI controller `[1]`, addr 8 = `05ac:8286`, FS, depth 3,
parent hub 5 port 3, TT = (hub 4 port 1). Boot must carry `UNAOS_BT=1`.

## 0. What Boot AM actually established

```
:: bt-l0: [1] addr 8 intf 1 class=0xE0/0x01/0x01 but NO interrupt-IN event endpoint — cannot read HCI events; claimed and stopped ::
```

Note what this line does and does not prove. `class=0xE0/0x01/0x01` is a **literal
in the old format string**, not a field read from the device — only `[1]`, `addr 8`
and `intf 1` were substituted. So the wire facts from Boot AM are exactly two:

1. some alt-0 interface with `bInterfaceNumber == 1` matched `0xE0/0x01/0x01`;
2. that interface had no interrupt-IN endpoint **within the first 64 bytes of the
   configuration descriptor**, which is all the old code could see.

Both convicted mechanisms are in that sentence, and both are fixed in this arc
(first-match-by-class ordering; 64-byte truncation of the descriptor). The census
is what turns the remaining unknown — the actual interface/endpoint layout of the
8286 — from a hypothesis into a printed fact.

## 1. Expected: the census

Printed for the BT function only, before any decision, one line per interface
**descriptor** (each alternate setting is its own line). Shape, verbatim modulo
the numbers:

```
:: bt-l0: [1] census addr=8 intf=0 alt=0 alts=1 class=0xff/0x01/0x01 neps=3 eps=[IN1/int/16 IN2/blk/64 OUT2/blk/64] == witness ::
:: bt-l0: [1] census addr=8 intf=1 alt=0 alts=6 class=0xe0/0x01/0x01 neps=2 eps=[IN3/iso/0 OUT3/iso/0] == witness ::
:: bt-l0: [1] census addr=8 intf=1 alt=1 alts=6 class=0xe0/0x01/0x01 neps=2 eps=[IN3/iso/9 OUT3/iso/9] == witness ::
… (alts 2..5, same shape, growing iso mps)
```

Field grammar: `intf` = bInterfaceNumber, `alt` = bAlternateSetting, `alts` = how
many descriptors share that bInterfaceNumber, `class` = the real class/subclass/
protocol triple read off the wire, `neps` = bNumEndpoints as declared, `eps` = the
endpoints actually parsed, as `DIR + number / kind / wMaxPacketSize` with kind in
`ctl|iso|blk|int`.

Load-bearing claims about the census, each independently checkable:

- **At least two lines appear.** One line only would mean the full-descriptor
  re-read did not happen or the device really has one interface.
- **`intf=1` shows `class=0xe0/0x01/0x01` and its endpoints are `iso`**, i.e. Boot
  AM's interface was the SCO interface, exactly as Bluetooth Core Vol 4 Part B
  describes it (isochronous, alt 0 zero-bandwidth). `alts` for it should be > 1.
- **`intf=0` shows an `int` IN endpoint plus a `blk` IN/`blk` OUT pair.** That is
  the HCI transport fingerprint. Its class byte is the open question: `0xe0` (the
  spec triple) or `0xff` (vendor-classed — which is what the device-level class
  `0xff` read on Boot AM predicts).
- If `wTotalLength` exceeded 256 B or more than 12 interface descriptors exist, an
  additional `census addr=8 INCOMPLETE` line says so. Absence of that line means
  the census is the whole device.

## 2. Expected: the corrected claim

Immediately after the census, exactly one of these two, naming the interface **and**
the endpoint it was selected by:

```
:: bt-l0: [1] claim addr=8 intf=0 alt=0 class=0xe0/0x01/0x01 evt_ep=IN1 -> selected by ENDPOINT EVIDENCE, tier 1 (spec: Bluetooth Core Vol 4 Part B class triple + interrupt-IN) == witness ::
```

or, if interface 0 is vendor-classed:

```
:: bt-l0: [1] claim addr=8 intf=0 alt=0 class=0xff/0x01/0x01 evt_ep=IN1 -> selected by ENDPOINT EVIDENCE, tier 2 (vendor-classed: RF/Bluetooth subclass+protocol + int-IN/bulk-IN/bulk-OUT HCI fingerprint) == witness ::
```

Tier 2 is deliberately not dressed up as a spec claim. The spec cite (Vol 4 Part B,
class 0xE0 / subclass 0x01 / protocol 0x01) covers tier 1 only; tier 2 is a
structural match on the RF/Bluetooth subclass+protocol pair plus the full HCI
endpoint set, and the line says so in its own words.

Then the pre-existing reachability line, now printing the real class triple rather
than a hardcoded one, and naming `intf=0` rather than `intf=1`:

```
:: bt-l0: [1] reachability addr=8 spd=FS intf=0 class=0x??/0x01/0x01 evt_ep=IN1 mps=16 interval=1 bulk_in=IN2 bulk_out=OUT2 parent=(hub 5 port 3) tt=(hub 4 port 1) -> TT-INHERITED(parent hub is not high speed; TT is the nearest HS ancestor) == witness ::
```

`tt=(hub 4 port 1)` must be unchanged from Boot AM. If it moved to `hub 5`, the
split-transaction addressing regressed and every HCI result below is void.

## 3. Expected: the radio answers

```
:: bt-l0: [1] HCI_Reset (0x0C03) -> CmdComplete status=0x00 -> OK == witness ::
:: bt-l0: [1] HCI local version — manufacturer=0x000f -> BROADCOM ::
:: bt-l0: [1] HCI_Read_Local_Version_Information (0x1001) -> HCI_Version=... LMP_Version=... == witness ::
```

`manufacturer=0x000f` is the deliverable: the Bluetooth SIG company identifier for
Broadcom cannot be produced by our own code, by a timing artefact, or by a hopeful
default. It can only have come off the radio.

The HCI_Version byte then dates the part (Bluetooth Core, Assigned Numbers):

- **`HCI_Version=0x04` (and `LMP_Version=0x04`) = Bluetooth 2.1 + EDR.** The
  BCM2046-class radio. No LE at all — a later LE arc would be dead on arrival and
  the roadmap for this machine is classic-only.
- **`HCI_Version=0x06` (LMP 0x06) = Bluetooth 4.0.** The BCM20702-class radio, LE
  present in ROM. This is what a 2012 rMBP is expected to carry, and it is the
  value that makes an LE-scan arc worth planning.

Anything else (0x05 = BT 3.0, 0x07 = 4.1, 0x08 = 4.2) is a real answer too — the
prediction that matters is that the byte is **non-zero and self-consistent with
LMP_Version**, since a stuck-at-zero pair would indicate a malformed event parse
rather than a radio reading.

## 4. Refutes

**R1 — the census shows NO interface with an `int` IN endpoint.**
Then the 8286 does not expose the HCI event transport the way this driver reaches
it, and the arc's premise is wrong, not its implementation. Expected line:
`bt-l0: [1] addr 8 — NO interface carries an interrupt-IN HCI event endpoint …
census above is the wire truth`. Next step is a different transport question (is
the event stream on a vendor bulk endpoint? is there a second configuration?), not
a tweak to the selector. The census is designed so that this refute arrives with
its own evidence attached.

**R2 — the census shows an `int` IN endpoint, but on an interface neither tier
matches** (e.g. class `0xff` but subclass/protocol not `0x01/0x01`, or missing the
bulk pair). Same stop line. The fix is a widened tier-2 fingerprint, and the census
line supplies the exact triple to widen it to. This is the cheap outcome: one more
boot, one more line of code.

**R3 — the claim succeeds but `HCI_Reset` returns NO-RESPONSE, or the read times
out.** Then the transport is right and the radio is not answering ROM-level
commands. That is **firmware (`.hcd` patchram) territory**, and it is out of scope
for this repo: Broadcom patchram blobs and their loading protocol land in
**UnaOS-bunker**, not in UnaOS. This repo's boundary ends at "the radio was reached
and did not answer"; the bunker's begins at "here is the blob and the loader". Do
not add a firmware path to `drivers/ehci/mod.rs` on the strength of this refute.

**R4 — regression tripwire.** The keyboard/hub walk that found addr 8 on Boot AM is
metal-proven and this arc must not have disturbed it. The capture must still show
the same enumeration order and the same `EHCI-HID` lines for addrs 1-7, and the
internal keyboard must still type. Any change there is a defect in this arc
regardless of what the BT lines say, because non-candidate devices take the
candidate gate's early return and issue no extra traffic at all.
