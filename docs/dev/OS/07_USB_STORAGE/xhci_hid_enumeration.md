# xHCI HID enumeration — the single arming walk

How the xHCI driver (`unaos/crates/kernel/src/drivers/xhci/mod.rs`) discovers and
arms Human Interface Devices — keyboards, mice, and tablets — during enumeration,
and why the arming logic is now a single shared walk (`record_hid_interfaces`)
used by both the root-port path and the hub-downstream path.

This doc is scoped to HID **interface discovery and arming**. The transport that
carries HID reports once armed (Configure-Endpoint, SET_PROTOCOL, the interrupt-IN
report ring, and the report → `pal::Event` decode) lives in `usb_xhci.md` and in
the mouse/keyboard sections of the driver.

---

## 1. Two devices, one composite receiver

A USB HID device advertises its interfaces in the configuration descriptor. The
driver supports **one keyboard** and **one pointer** per device. Three shapes
matter:

| Device | Interfaces | bInterfaceProtocol |
| --- | --- | --- |
| `usb-kbd` (QEMU) | one HID | 1 (boot keyboard) |
| `usb-tablet` (QEMU) | one HID | 0 (absolute pointer) |
| Composite receiver (e.g. Logitech Unifying `046d:c534`) | two HID: iface0 keyboard, iface1 mouse | iface0 = 1, iface1 = 2 (boot mouse) |

The composite receiver is the case that exposed the bug: it is a **single USB
device** that enumerates as **two HID interfaces**. Arming only the first
interface arms the keyboard and silently drops the mouse.

### Protocol disambiguation

A composite device can expose keyboard, mouse, *and* a consumer/system-control
interface. The driver must not mistake a proto-0 consumer-control interface for a
pointer and clobber a real mouse. The rule (identical on both paths):

- **proto 1** — boot keyboard. Record the first; ignore later keyboard interfaces.
- **proto 2** — boot mouse. The definitive relative pointer; **always wins**,
  overriding an earlier ambiguous proto-0 pointer on the same device.
- **proto 0** — ambiguous (`usb-tablet` OR consumer-control). Accepted as an
  **absolute** pointer only if no pointer has been recorded yet, so a later proto-0
  cannot overwrite it and a real proto-2 mouse still overrides it.

Only one interrupt-IN endpoint is taken per interface; the internal `found_hid`
flag is cleared after each armed endpoint so a trailing endpoint descriptor on the
same interface cannot re-arm it.

---

## 2. The two historical paths — and the bug

Before this work there were **two independent HID-arming implementations**:

### Root-port path — armed every interface (correct)

When a device is enumerated directly on a root port, its configuration descriptor
arrives as a descriptor event. The inline walk in the event handler
(the `bDescriptorType == 0x02` branch, around `mod.rs:1730`) iterated **every**
interface, recorded both `is_keyboard`/`keyboard_ep` and `is_mouse`/`mouse_ep`
with the proto-0/1/2 disambiguation above, and then issued **one**
Configure-Endpoint covering all armed HID endpoints. A composite receiver plugged
directly into a root port therefore armed keyboard **and** mouse — the mouse
tracked (`MOUSE-1` witness fired).

### Hub-downstream path — armed the first interface only (the bug)

When a device sits behind a hub, `enumerate_downstream` (around `mod.rs:4905`)
reads the first 64 bytes of the configuration descriptor and previously armed HID
via `parse_hid_config`, a helper that **stopped at the first interrupt-IN
endpoint** (`return Some(...)`). For the composite receiver the first hit is
iface0 (the keyboard), so the hub path wrote only `is_keyboard`/`keyboard_ep` onto
the slot and **never set `is_mouse`**. The mouse was silently dropped.

Grounded observation (2026-07-17 attended rMBP six-knob sitting): the Logitech
Unifying receiver `046d:c534` armed **keyboard-only** behind the VIA SuperSpeed
hub `2109:0813`, while the same receiver plugged directly into a root port armed
keyboard + mouse and tracked. "Just works on other OSes" — the two-path split was
the only reason it did not here.

---

## 3. The shared walk — `record_hid_interfaces`

The fix collapses the two implementations into one method (around `mod.rs:5106`):

```rust
fn record_hid_interfaces(&mut self, slot_id: u8, buf: u64) -> bool
```

It walks a configuration descriptor (`buf`, clamped to a 64-byte window like
`parse_msc_config`) and records **every** HID interrupt-IN interface onto
`slots[slot_id]` with the proto-0/1/2 disambiguation from §1, returning `true` iff
at least one interrupt-IN endpoint was armed. The descriptor is read through a
bounds-clamped raw pointer (`buf as *const u8`); the method writes only the HID
slot fields, so it is safe to call while the caller still holds a separate
raw-pointer view of the same buffer (both are reads of that memory).

Both paths now route through it:

- **Root port** (around `mod.rs:1815`): after the inline walk collects the
  MSC/FTDI bulk endpoints, HID is armed by
  `if self.record_hid_interfaces(slot_id, desc_buf as u64) { self.configure_hid_endpoints(slot_id, true); }`.
  The MSC/FTDI bulk-collection loop is unchanged.
- **Hub downstream** (around `mod.rs:5049`): the old
  `match self.parse_hid_config(buf)` block is replaced by
  `if self.record_hid_interfaces(slot_id, buf) { self.configure_hid_endpoints(slot_id, false); } else { /* no HID interrupt endpoint */ }`.

`parse_hid_config` (single-interface, first-endpoint-only) is deleted. There is
now exactly **one** HID-arming walk.

The `root_fsm` argument to `configure_hid_endpoints` (`true` on the root path,
`false` on the hub path) selects the completion bookkeeping; the arming walk itself
is identical.

---

## 4. Everything downstream was already multi-interface

The reason the fix is localized to the descriptor walk: the entire pipeline after
the slot-state write already handled keyboard + mouse together.

- **`configure_hid_endpoints`** (around `mod.rs:5505`) programs the keyboard *and*
  the mouse endpoint in **one** Configure-Endpoint command and sets
  `keyboard_state`/`mouse_state`.
- The **Configure-Endpoint completion** (around `mod.rs:1377`, keyed on slot
  state, "root FSM or hub bring-up") is path-agnostic.
- The **SET_CONFIGURATION completion** (around `mod.rs:1639`) queues **both** the
  keyboard and the mouse interrupt-IN reads and emits the
  `:: MOUSE-1: HID pointer detected ... == witness ::` line.
- **`service_hid_setproto`** (around `mod.rs:4147`) and the transfer dispatch are
  likewise multi-interface.

So arming `is_mouse` on the hub slot is sufficient — the rest of the pipeline arms
itself. This is why the change is a walk-unification, not a pipeline rewrite.

---

## 5. Verification

QEMU coverage of the shared walk:

- **x86_64 (`./arroyo test`)** — root topology is `usb-storage` + `usb-kbd` +
  `usb-tablet` on `xhci.0`. The tablet exercises the proto-0 pointer branch
  (`POINTER INTERRUPT IN EP FOUND ... ABSOLUTE tablet (proto 0)` + `MOUSE-1`);
  storage reaches `MISSION SUCCESS`. These root witnesses are **byte-identical**
  before and after the walk-unification (the extraction is behavior-preserving).
- **aarch64 (`./arroyo test-arm`)** — the virt topology enumerates the keyboard
  and the tablet on the root xHCI, so the shared walk arms **both**: the
  `KEYBOARD INTERRUPT IN EP FOUND` (proto 1) *and* `POINTER INTERRUPT IN EP FOUND
  ... ABSOLUTE (proto 0)` witnesses fire together, plus `MOUSE-1`, `MISSION
  SUCCESS`, no panic. This directly exercises the multi-interface keyboard + mouse
  arming.

### What QEMU cannot cover

QEMU's `usb-kbd` and `usb-tablet` are **separate single-interface devices**, and
the test topology attaches them to the root bus, not behind a hub. QEMU cannot
build a **composite** kbd+mouse device **behind a hub**, so the exact failure the
fix targets — a composite receiver behind a hub arming both interfaces — is **not**
reproducible under QEMU.

### Owed metal proof

The composite-behind-hub → mouse-tracks confirmation is a **later attended rMBP
metal leg**, explicitly out of this arc's QEMU DONE gate. The reproducer is the
**2026-07-17 rMBP six-knob attended sitting**: the Logitech Unifying receiver
`046d:c534` behind the VIA SuperSpeed hub `2109:0813`. The pass criterion is that
the receiver behind the hub arms keyboard **and** mouse (both slot fields set) and
the mouse tracks (signed `dx`/`dy`, the `MOUSE-1` witness), matching the
direct-to-root-port behavior already confirmed at that sitting.
