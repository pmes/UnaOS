# I/O APIC — the redirection table, and the end of "there is no IOAPIC in this kernel"

**Subsystem:** x86_64 interrupt plumbing. Shared by every x86 machine this OS boots — QEMU q35 and
the 2012 rMBP alike. Nothing here is board-specific.

**Code:** `unaos/crates/kernel/src/arch/x86_64/ioapic.rs` · the MADT half in
`unaos/crates/kernel/src/arch/x86_64/acpi.rs`.

**Knob:** `UNAOS_IOAPIC=1` → Cargo feature `ioapic`. Default OFF, byte-identical
(`./arroyo knoboff ioapic`). **Ledger:** rmbp `B147`.

**Clean room.** Intel 82093AA I/O APIC datasheet (§3.1 the IOREGSEL/IOWIN window, §3.2.1 IOAPICID,
§3.2.2 IOAPICVER, §3.2.4 the 64-bit redirection entry) · Intel SDM Vol. 3 §10 (local APIC, delivery
modes, §10.8.5 the EOI broadcast that clears Remote IRR) · ACPI 6.x §5.2.12 (MADT; §5.2.12.3 I/O
APIC, §5.2.12.5 Interrupt Source Override and the MPS INTI flags, §5.2.12.7 Local APIC NMI) · PCI
Local Bus 3.0 (§2.2.6 INTx is level-triggered and active low, §6.2.2 COMMAND bit 10 Interrupt
Disable, §6.2.4 Interrupt Line / Interrupt Pin). All public; no Linux source was read.

**Rungs.** 1 `ioapic-census` (this document's §2) · 2 `ioapic-route` · 3 `ioapic-ehci`. Each lands
green before the next, and this file grows with them.

---

## 1. The defect

This kernel has had local APICs since the beginning and MSI/MSI-X since the xHCI arc
(`drivers/pci.rs::enable_msi` / `enable_msix`). It has never had an I/O APIC. `arch/x86_64/apic.rs`
is 498 lines of local APIC — xAPIC MMIO at `0xFEE00000` and the x2APIC MSR bank — and contains no
redirection-entry writer of any kind. `interrupts::disable_legacy_pic` masks every 8259 line, and
`apic::init` leaves LINT0 masked, so there is no legacy virtual-wire path either.

The consequence is exact: **a PCI function that offers no usable MSI capability cannot deliver an
interrupt at all.** It can assert INTA#, and the assertion goes nowhere. The only thing a driver can
do with such a function is poll it.

The measured case is the EHCI controllers. rMBP flight 11, `f11.log` at 25626 ms:

```
:: EHCI-HID: [1] ISRARM REFUSED — this function offers no usable MSI capability, and there is
no IOAPIC in this kernel to route INTx to. The endpoint stays on the POLLED re-arm path, which
is unchanged; the dark window EHCIDARK measures is unchanged with it == witness ::
```

That refusal was correct in every word. The cost it names is the internal trackpad's: ledger `B139`
records `EHCIDARK … kind=vendor-mt reports=5706 … dark=13646ms max=108ms missed<=11384` over a
~970 s boot — dark windows up to 108 ms, and an upper bound of ~11k reports lost on the wire.

## 2. Rung 1 — the census

`acpi::parse_madt` walks the MADT's variable-length entry list and matches exactly two types: Local
APIC (0) and Local x2APIC (9). Everything else falls into `_ => {}`. **That catch-all is where the
I/O APIC (type 1), the Interrupt Source Override (type 2) and the Local APIC NMI (type 4) entries
have been going since the file was written.**

The arm now forwards every unconsumed entry to `ioapic::madt_entry`, which picks out those three and
does its own type and length checks. The topology walk itself is untouched — it is still the
local-APIC list it was, which is the point of collecting elsewhere rather than widening the match.

`ioapic::census`, called from `acpi::init` immediately after the walk, then does the hardware half:
for each declared controller it calls `memory::map_mmio_window(addr, 0x1000)`, selects `IOAPICVER`
through IOREGSEL and reads IOWIN.

Three facts worth stating, because they are the usual ways this goes wrong:

- **IOAPICVER bits 23:16 are the *maximum redirection entry* — the last valid index — so the entry
  COUNT is that field plus one.** Getting this off by one is how a kernel silently refuses the
  highest GSI on the machine.
- **`0xFFFFFFFF` is what an unclaimed MMIO read returns.** A controller answering that, or claiming
  a max-entry of `0xFF`, is refused with `reason=ioapicver-unreadable` and left with `entries = 0`,
  which makes it structurally unroutable when rung 2 arrives.
- **`map_mmio_window` is called rather than assumed.** The LAPIC window is reached raw because UEFI
  identity-maps it, and this one sits 2 MiB below in the same firmware-reserved aperture — but an
  address firmware handed us is not an address we have seen mapped. The call asserts the UC typing
  the register pair requires and creates the leaf if the boot map lacks it, turning a would-be #PF
  into a normal read. It is safe at this point in the boot: `acpi::init` runs long after the heap,
  so the frame allocator the window walk may need is live.

**Nothing is written this rung.** The wire:

```
[ioapic] id=<n> addr=<phys> gsi_base=<n> entries=<n> version=<v> hw_id=<n>
[ioapic] iso bus=<n> irq=<n> -> gsi=<n> polarity=<p> trigger=<t> flags=<hex>
[ioapic] nmi uid=<n> lint=<n> polarity=<p> trigger=<t>
[ioapic] census ioapics=<n> isos=<n> gsis=<total> nmis=<n> dropped=<n> madt_entries=<n>
```

`dropped=` is the count of entries the fixed-size tables could not hold (4 controllers, 16
overrides, 8 NMI entries). It is printed rather than left implicit: a machine larger than this
module's static capacity says so instead of quietly losing an entry.

A machine whose MADT declares no I/O APIC at all prints the `ioapics=0` census line and says in it
that every function without MSI stays polled. **Silence is never the answer** — the census line is
unconditional.

`Polarity::BusDefault` / `Trigger::BusDefault` are carried as their own values rather than flattened
to active-high/edge, because the ACPI encoding `00` means "whatever this bus defaults to" and what
that is depends on the bus. Resolving it is the caller's job, and rung 2 is the first caller.

## 3. Byte identity

`arch/x86_64/mod.rs` declares `pub mod ioapic;` at the **file tail**, and that is a byte-identity
requirement rather than style: a `pub mod` inserted anywhere above shifts every `panic::Location`
line below it.

The two `acpi.rs` sites are `#[cfg]`-erased **line-neutral** appends with the code before the `//`
(LEDGER P7 — after the slashes the statement is a comment, compiles nothing, and the check stays
green). `acpi.rs` is 557 lines before and after.

On aarch64 the feature is stripped by arroyo's `arm_features`, exactly as `hda` and `ahci` are: the
module lives in a directory aarch64 never compiles, its call sites are x86-gated, and aarch64 routes
its interrupts through the GIC. An enabled feature is still hashed into the build fingerprint even
when it compiles nothing, so leaving it in would shift every Pi and Jetson media hash for zero
observable change.

## 4. What flight 12 should print on the rMBP

The 7-series (Panther Point) PCH is expected to carry **one I/O APIC at `0xFEC00000` with 24
redirection entries**. That is an expectation stated here so the flight can contradict it — the
census line is the measurement, not this paragraph.

Also wanted from that boot, and recorded in `B147`: the EHCI functions' **Interrupt Line** values as
the wire reports them (a `255` is itself a finding — firmware routed the function nowhere), and what
happens to the `EHCIDARK` line when a completion interrupt can finally arm.
