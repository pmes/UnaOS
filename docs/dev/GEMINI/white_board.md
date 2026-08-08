# WHITE BOARD — 2026-08-08 (GR21)

## Q4 — Bluetooth: is the goal WIRELESS INPUT, or BLUETOOTH the subsystem?

Background: BT recon (`~/unaos-bench/scratch/gr21/bluetooth-recon.md`) found the radio —
a Broadcom BT module behind its own integrated USB hub (`0a5c:4500` = "BCM2046B1 … part of
BCM2046 Bluetooth"), at USB depth 3, **which our enumerator has never reached** (the EHCI
depth cap stops one tier short, by design, sized to reach the keyboard). The HCI
controller's exact VID:PID is in no capture we hold — the recon refused to fill that from a
spec sheet.

The recon **disagrees with the seat's own earlier framing** and puts it on record: BT-HID
(a Bluetooth keyboard) is the *most* expensive input path, not the cheap one — it needs the
whole stack (HCI → L2CAP → SDP → pairing → HIDP, ~4000–6000 LOC over many arcs), **plus
bulk transfers on silicon whose async schedule master-aborts (PROBE-14)**, plus crypto the
kernel doesn't have. **A 2.4 GHz USB dongle gives a wireless keyboard TODAY with zero new
code** — the xHCI HID path already handles composite dongles. So:
- If the goal is *wireless input*: it's already met; BT is optional.
- If the goal is *Bluetooth as a subsystem* (audio, phones, BLE peripherals): it's a real
  multi-arc project and worth starting — but as its own track, sized honestly.

**Seat recommendation: take the ONE recon arc regardless** (below), then decide scope from a
measured `HCI_Version` rather than a guess.

## Q5 — The BT recon arc: approve it? (one arc, fixes a real latent bug either way)

Background: the first arc is L0 — lift the depth cap to reach depth 3, recognize class 0xE0,
issue `HCI_Reset` + `HCI_Read_Local_Version_Information` over the control endpoint. It needs
**no new transfer machinery** (control-with-TT-split and periodic interrupt-IN are both
already metal-proven on this controller). Witness: `Manufacturer_Name == 0x000F` (Broadcom)
— a value nothing but the real chip can produce. Knob-gated `UNAOS_BT=1`, default off.

The arc is worth doing **even if BT is then shelved**, because it fixes a latent USB bug the
depth cap is currently masking: the split-transaction TT is computed as "this hub's," but
the Broadcom hub is full-speed with no TT, so devices below it are served by the SMSC hub's
TT — a naive depth lift would program the wrong TT and read the radio as dead. ~4 lines, and
it must land in the same arc. It also converts "probably Broadcom BCM2046" into a fact (the
recon flagged that the hub trains at *full* speed, which fits the older BT 2.1 BCM2046, not
the BT 4.0 BCM20702 usually quoted for this machine).

Sub-decisions, only if the arc runs and P7 falsifies (needs firmware):
- The `.hcd` patchram blob is **probably a non-issue** — `HCI_Reset`/`Read_Local_Version`
  are ROM-level; Linux runs on ROM firmware when the blob is absent. Bunker treatment is
  already specified if we ever need it.
- Does macOS still exist on the rMBP's SSD? (Only matters if we ever need to extract a blob.)

---

(Q1 answered — Claude takes DISPLAY; igpu continues; kepler stays FENCE; GEN7 not a lane;
blob work → private UnaOS-bunker repo, which now exists.)
(Q2 answered — write on; SD write ships once 4c is proven; UnaFS already exists as the FS.)
(Q3 answered by flight — internal-slot boot works; both one-volume-collapse HW gates open.)
