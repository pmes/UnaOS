# IDT vectors — the allocator, and the end of picking the next free number by eye

**Subsystem:** x86_64 interrupt plumbing. Shared by every x86 machine this OS boots — QEMU q35 and
the 2012 rMBP alike. Nothing here is board-specific.

**Code:** `unaos/crates/kernel/src/arch/x86_64/interrupts.rs` — the `vectors` module, the three
reservations and the three seed allocations in the IDT initializer · the two MSI-enable sites in
`unaos/crates/kernel/src/arch/x86_64/pci.rs` · the one MSI/INTx site in
`unaos/crates/kernel/src/drivers/ehci/mod.rs::isr_arm_controller`.

**Knob:** NONE, deliberately. The allocator replaces unconditional code with unconditional code, so
there is nothing to arm and **no `./arroyo knoboff` verdict may be cited for it** (LAWS §5,
2026-09-22). The proof is the replay: the same vectors delivered, the same counters moving, plus a
census line that did not exist before. **Ledger:** rmbp `B168`. **Spec pin:**
`unaos/scripts/specs/x86-default.spec` (tail).

**Clean room.** Intel SDM Vol. 3A §6.1–§6.2 (the IDT, the 32 reserved exception vectors, the gate
descriptor) · §10.8.3 (the task-priority/vector relationship: a vector's priority class is its
number >> 4) · §10.9 (the spurious-interrupt vector and the SVR low byte). All public; no Linux
source was read.

---

## 1. The defect

Until this arc the x86 kernel numbered its interrupt vectors **by hand**, in five `pub const`s at
the top of `interrupts.rs`:

```rust
pub const TIMER_VECTOR: u8    = 0x20;
pub const XHCI_MSI_VECTOR: u8 = 0x40;
pub const NIC_MSI_VECTOR: u8  = 0x41;
pub const IPI_VECTOR: u8      = 0x42;   // "0x41 is reserved for the NIC, so IPIs use 0x42"
pub const EHCI_MSI_VECTOR: u8 = 0x43;   // "0x40-0x42 are taken … so the EHCI functions share 0x43"
```

Each was registered by name in the IDT, and each was named a second time by the driver that
programmed its MSI (`pci.rs` for the NIC and the xHCI, `drivers/ehci/mod.rs` for the EHCI) or — since
IOAPIC (`B147`) — its redirection entry. **The comments above are the whole allocation policy**: a
human read the list and picked the next number.

That is one bug away from a defect with no witness. `idt[v].set_handler_fn(handler)` is a **plain
overwrite**. Two drivers that pick the same number both succeed; the second registration wins; the
first ISR never runs again. On the wire that is *indistinguishable from a device that never
interrupted* — the same silence, the same counters at zero, the same "the interrupt is dead" hunt
that IOAPIC's rung 3 had to run from scratch.

The next device that needs a number is not hypothetical. The HDA controller's completion (`B152`'s
1210 ms tone test runs entirely polled), the AHCI port (`B89`), a second EHCI or xHCI function, the
UVC isochronous pipe when CAMERA2 opens it — each would have copied the pattern. And the rmbp-queue
KVBLANK row states the same defect from the GPU side, in its own words:

> `mode=poll` is a FINDING: `arch/x86_64/interrupts.rs` has three hard-coded IDT vectors and **no
> allocator a PCI function can join**.

This arc builds the allocator. It does not, by itself, make the Kepler vblank an interrupt — that
row's other blocker (the PDISPLAY ENABLE/STATUS pair is not in the tree) is untouched.

## 2. The allocator

`interrupts::vectors`. One table, one mutex, no heap — it runs at `init_idt`, long before
`memory::init`.

| item | value | why |
|---|---|---|
| `RANGE_LO` | `0x30` | above the 32 Intel exception vectors and above the 0x20–0x2F band this kernel gives the APIC timer |
| `RANGE_HI` | `0xEF` | below the 0xF0–0xFF band the SVR's spurious vector lives in |
| `FLOOR` | `0x40` | the lowest number `alloc` will hand out — see §3 |
| reserved by name | `timer` 0x20 · `ipi` 0x42 · `spurious` 0xFF | numbers this kernel cannot move: `apic.rs` arms the timer at `TIMER_VECTOR`, `smp.rs`/`sched.rs` build the ICR word from `IPI_VECTOR`, and `SPURIOUS_VECTOR` **is** the SVR low byte |

Four calls:

* `reserve(vector, name)` — record ownership of a number the kernel cannot move. It installs
  nothing; the three reserved handlers are set in the IDT initializer exactly as they always were.
* `alloc(name, handler) -> Option<u8>` — the lowest free vector at or above `FLOOR`, **registered in
  the IDT before the number is returned**, so a caller can never program a device with a vector
  whose entry is not yet live. Refuses a duplicate name and a full range, each with a witness.
* `of(name) -> Option<u8>` — how a driver asks. There is no const left to name.
* `census()` — the one line, §4.

**`alloc` is strict about names on purpose.** A second `alloc` under a name that already owns a
vector is exactly the collision of §1, arriving through the front door, so it is refused and said
out loud rather than served. Two EHCI controllers still *share* one vector — they always did, and
deliberately: MSI carries no cause, the handler acknowledges every armed controller's `USBSTS`
anyway, and a second vector would buy nothing but another IDT entry. They share it by both **asking**
(`of("ehci")`), not by both allocating.

**The IDT is now mutable after `lidt`**, which is what makes the allocator joinable by a driver that
probes late rather than only by `interrupts.rs`. It lives in an `UnsafeCell` (`IdtCell`) and
`init_idt` publishes the pointer. Writing a descriptor of a loaded IDT is defined: the CPU reads the
entry at **delivery**, and the entry `alloc` writes is not deliverable until its caller programs the
device with the number `alloc` just returned. Single-writer by construction — the BSP builds the
table, the `TABLE` mutex serialises the choice of slot, and each slot is written exactly once.

## 3. Why the wire does not change, and why that is a requirement

The three device vectors this kernel already had are **the allocator's first three answers**, seeded
in the IDT initializer in this order:

```
reserve timer 0x20 · reserve ipi 0x42 · reserve spurious 0xff
alloc "xhci" -> 0x40      (lowest free at or above FLOOR)
alloc "nic"  -> 0x41
alloc "ehci" -> 0x43      (0x42 is already owned by `ipi`, so the third answer skips it)
```

Same numbers, same order. Every capture on this bench — every `[ioapic] armed … vector=0x43`, every
`[e1000] RX interrupt (MSI vector 0x41)`, every `ISRARM armed — MSI vector 0x43` — stays
**byte-comparable across the fold**, which is the only thing that lets a regression in the interrupt
path be *seen*. A later arc may renumber; it must say so and re-pin the captures in the same commit.

**The seeding site is the only place that order can be pinned, and that is a measurement rather than
a preference.** The drivers probe in the order **EHCI → xHCI → NIC** (`pci::init` calls
`ehci::init` at its SDHC/HDA block, the xHCI scan below it, and `init_network()` last). An allocator
asked at each driver's own MSI site would therefore have answered `ehci` 0x40, `xhci` 0x41,
`nic` 0x43 and moved three numbers on the wire for nothing.

**`FLOOR` is 0x40, not `RANGE_LO`**, for that reason and for one more: a vector's priority class on
x86 is its number >> 4 (SDM Vol. 3 §10.8.3), so 0x30–0x3F is where a *deliberately low-priority*
device belongs — not where the next arrival lands by accident. The band is held; releasing it is a
decision, and it renumbers nothing when it is taken.

## 4. The census

One line, printed from the IDT initializer, sorted by vector:

```
[vectors] allocated=3 free=172 table=timer:0x20,xhci:0x40,nic:0x41,ipi:0x42,ehci:0x43,spurious:0xff == witness ::
```

* `allocated=` counts `alloc` successes. The three reserved names appear in `table=` and not in that
  count — a reservation is a number the kernel was born with, and folding the two would make the
  field unreadable.
* `free=` is what `alloc` could **still** hand out: the unowned slots from `FLOOR` to `RANGE_HI`.
  176 slots in `0x40..=0xEF`, less the four owned (0x40, 0x41, 0x42, 0x43) = **172**. It answers "how
  many more devices fit", not "how wide is the range".
* `table=` is the assertion. A number moving there is the regression this arc exists to make
  visible, and it is what `x86-default.spec` pins character for character.

**Where it is printed, and why not later.** Every vector this kernel hands out is allocated while
the IDT is *constructed* — that is what pins the numbers — so the table is complete at `init_idt`,
and printing it there makes the line **unconditional**: it reaches the wire of the default image
that boots the metal whether or not a NIC, an xHCI or an EHCI was ever found. A census printed from
a driver's own arm site could not promise that (the rMBP's NIC is a Broadcom part that takes
`init_network`'s non-Intel early exit on every boot, well before the MSI-enable site). A runtime
`alloc` re-prints the census after its own line, so the **last** `[vectors] allocated=` line of any
capture is always the final table.

Knob-off `ehcihid` the `ehci` name is simply absent and the census says so; the other five are
unconditional.

## 5. What a driver's site looks like now

`pci.rs`, both MSI-enable sites, and the shape is the same at each:

```rust
match crate::arch::interrupts::vectors::of("nic") {
    Some(vector) => { let _ = e1000::enable_interrupts(bus, slot, func, msg_addr, vector as u32); }
    None => serial_println!(":: x86_64 PCI: NIC MSI NOT ARMED — … == witness ::"),
}
```

`drivers/ehci/mod.rs::isr_arm_controller` takes its answer once, at the top, as a **same-line fold**
(`B94`) so the file stays line-neutral (18921 lines before and after — `panic::Location` records
embed line numbers in that 18k-line file and B147's own byte-identity claim depends on them):

```rust
let ehci_vec = match vectors::of("ehci") { Some(v) => v, None => { /* ISRARM REFUSED … */ return; } };
if !PciScanner::enable_msi(bus, dev, func, msg_addr, ehci_vec as u32)
    && !arch::x86_64::ioapic_route_intx(bus, dev, func, ehci_vec) { … }
```

So **IOAPIC's `route_pci_function` now takes an allocated vector**, not a hand-written const
(`ioapic.md` §5 carries the note). The `None` arm cannot be reached on a built kernel — the seed is
unconditional and carries the same `ehcihid` gate this whole file does — and it exists so a driver
can never program a number nobody registered a handler for. That delivery would land on whatever
handler happens to sit at the vector, which is §1's collision reached from the other end.

## 6. Scoring, and the go-red

**The proof is a REPLAY, not a knob.** LAWS §5 (2026-09-22) forbids citing knob-off byte identity
for a change to unconditional code, and this change *is* unconditional code. So: the same vectors
delivered, the same counters moving, on the same lane, plus one line that did not exist before.

**Default lane, `./arroyo test 240`.** Base capture taken at the branch parent `2495f3a2` before a
line of this arc existed; the after capture is the committed tree, same verb, same fixture. Both
`TEST_RC=0`; the spec goes **6/6 → 7/7**, the seventh being this arc's own pin.

| line | base (`2495f3a2`) | after |
|---|---|---|
| `[e1000] RX interrupt (MSI vector 0x41): enabled` | present | present — **same vector** |
| `[e1000] RX #<n> … irqs=<n>` (last) | `#18 … irqs=18` | `#36 … irqs=30` |
| `xHCI: [IRQ] xHCI interrupts taken so far:` | `35` | `30` |
| `:: EHCI-HID: [0] ISRARM REFUSED — this function offers no usable MSI capability…` | present | present, **character for character** |
| `[vectors] allocated=…` | **absent** | `allocated=3 free=172 table=timer:0x20,xhci:0x40,nic:0x41,ipi:0x42,ehci:0x43,spurious:0xff` |

**The two counters are NOT byte-comparable and saying they were would be a lie about the
instrument.** They count packets and completions over a boot whose length is set by host load: the
base run reached completion at **+24.8 s** and the after run at **+103.5 s** on a bench at load 20+,
which is the whole of the 18 → 36 RX packets. What IS comparable, and what this table asserts, is
that both ISRs are armed at **the same vector**, that both counters **move off zero**, and that
`irqs=` still tracks `RX #` with the same small lag (the polled drain's). A dead vector reads
`irqs=0` with `RX #` climbing, and neither capture does.

The EHCI row is the sharper one: on the default lane QEMU's `hcd-ehci` advertises no PCI capability
list and `ioapic` is off, so `isr_arm_controller` refuses — and it refuses with the **pre-existing**
text, not the new `None` arm. The allocated vector existed; the MSI capability did not. That is the
`None` arm being unreachable, measured rather than asserted.

**IOAPIC replay (`B147`'s own lane), which is where the EHCI vector is actually used.**
`UNAOS_IOAPIC=1 UNAOS_QEMU_EXTRA="-qmp tcp:127.0.0.1:4493,server,nowait" ./arroyo test 150` with
`scripts/qmp_type.py --port 4493 --marker ':: EHCI-HID: [0] M2 armed keyboard' --bursts 6 --burst 9`
— B147's invocation on a port of its own, `TEST_RC=0`:

```
[ioapic] census ioapics=1 isos=5 gsis=24 nmis=1 dropped=0 madt_entries=7 == witness ::
[ioapic] route bdf=0:3.0 pin=INTD line=11 -> gsi=11 via=iso polarity=active-high trigger=level == witness ::
[ioapic] armed bdf=0:3.0 gsi=11 vector=0x43 dest_apic=0 entry=0x0000000000018043 unmasked_lo=0x00008043 intx_disable=0 routed=1 == witness ::
:: EHCI-HID: [0] ISRARM armed — MSI vector 0x43 -> apic 0xfee00000, USBINTR 0x00000000 -> 0x00000001 (USBINT unmasked) …
:: EHCI-HID: ISRARM armed=1 refused=0 irq=72 isr_rearm=64 poll_rearm=8 depth_max=8 ringfull=8 cont_isr=0 cont_poll=0 oversize=0 == witness ::
```

The `armed` line is **byte-identical to the one B147 recorded**, `entry=` and `unmasked_lo=` and
all — except that `vector=0x43` is now an allocated number rather than a const, which is the whole
claim. `irq=72 isr_rearm=64` is B147's own pass criterion (`irq=` off zero, most re-arms coming from
the ISR) and it is met. One `ISRARM IRQ DEAD … in 6033 ms (irq=0 …)` precedes them, as it must: the
self-check fires while the controller is armed and the typist has not started yet. B147's **go-red**
was that line *twice* with **zero** rollups; here it is once, followed by three rollups.

**GO-RED (scratch probe, run and reverted before the commit).** A `vectors::scratch_probe()` at the
end of `init_idt`, which does the collision **and then the same ask through the allocator**:

```
[vectors] SCRATCH before-allocator: idt[0x40] handler 0x3c53c5f0 -> 0x3c53c3e0 — TWO NAMES ON ONE
VECTOR (xhci, then a second driver that picked 0x40 by eye). The overwrite SUCCEEDED, xhci's ISR
will never run again, and nothing on the wire says so == witness ::
[vectors] SCRATCH restored: idt[0x40] handler 0x3c53c5f0 (== 0x3c53c5f0) == witness ::
[vectors] alloc name=xhci -> REFUSED reason=duplicate-name held=0x40 kind=allocated — that name
already owns a vector, and a second registration would OVERWRITE its IDT entry: one ISR would stop
running and the wire would look exactly like a device that never interrupted == witness ::
[vectors] SCRATCH after-allocator: alloc("xhci") -> None — the same ask, refused == witness ::
```

The two handler addresses are the whole argument: `set_handler_fn` **succeeded**, the entry moved
from `xhci_msi_handler` to `nic_msi_handler`, and the pre-allocator kernel would have printed
nothing at all. The allocator refuses the same ask and says which mistake was made
(`kind=allocated` vs `kind=reserved`).

**SPEC GO-RED.** The new `x86-default.spec` rule replayed against the base capture — the run that
has no allocator in it — `rc=1`, `6/7`, and it names itself:

```
FIRST-SHORTFALL x86-default.spec:139 REQUIRE \[vectors\] allocated=[0-9]+ free=[0-9]+ table=timer:0x20,xhci:0x40,nic:0x41,ipi:0x42,ehci:0x43,spurious:0xff == witness ::
```

There is no FORBID partner, and the reason is `B160`'s measurement rather than taste: a FORBID that
can no longer match reads ✅ with 0 hits, indistinguishable from one that passed. The failure mode
here is a vector silently changing, which a REQUIRE on the exact table convicts and no negation
could state more sharply.

## 7. What this does NOT do

* **It does not renumber anything.** That is §3, and it is the point.
* **It does not give any device a new interrupt.** No driver gains a vector in this arc; three
  drivers stop naming a const and start asking. The HDA, AHCI and UVC paths are still polled — they
  now have somewhere to join.
* **It does not free the 0x30–0x3F band.** Held, with a reason (§3), and releasing it is its own
  decision.
* **It does not make the census reflect a device that arms after `init_idt` without allocating.** A
  driver that programs an MSI with a number it invented is still possible — nothing in the CPU stops
  it. What is gone is the *reason* to: there is no const to copy and there is a table to ask.
* **`drivers/gpu/kepler_vblank.rs:59` still names the deleted consts in a module comment**
  (`XHCI_MSI_VECTOR` 0x40, `NIC_MSI_VECTOR` 0x41, `EHCI_MSI_VECTOR`). That file belongs to the
  KVBLANK arc and was not touched here; the numbers it quotes are still right.
