# IOAPIC3 (B216, R68) — the IRQEN-set PIRQ routes to its APIC-mode input; no chipset write

Flight 13, the rMBP (`docs/dev/evidence/rmbp-0915/flight13/f13-boot1.log`, 5837 ms):
```
[ioapic] pirq bdf=0:31.0 id=8086:1e57 family=pch7 rcba=0xfed1c000 pirqa=0x80 … pirqh=0x80 fn=0:29.0 pin=INTA d29ir=0x3236 -> pirq=G REFUSED reason=pirq-disabled
[ioapic] route bdf=0:29.0 pin=INTA line=0 -> REFUSED reason=pirq-disabled
:: EHCI-HID: [1] ISRARM REFUSED reason=pirq-disabled — MSI: this function offers no usable MSI capability; INTx: the I/O APIC route was refused …
```
Peter, 2026-09-24 (R68): "yes. write whatever is needed to the chipset to enable functionality."

## What the datasheet says the enabling step is
- `PIRQ[n]_ROUT` bit 7 (IRQEN) steers the **8259** path only; the note under it: BIOS clears it for PIRQs in use during POST, and
  the OS **sets** it again when it moves to I/O APIC delivery. A firmware that boots in APIC mode leaves 0x80 everywhere.
- APIC Interrupt Mapping: I/O APIC inputs 16-23 receive PIRQA#-PIRQH#, active-low, **regardless** of `PIRQ[n]_ROUT`.
- Clearing IRQEN would add a legacy 8259 delivery beside the APIC one — a second path nothing services.

## What changed (`arch/x86_64/ioapic.rs`, rung 4)
The IRQEN=1 arm returns `Ok(16 + idx)` and prints
```
[ioapic] pirq bdf=0:31.0 id=8086:1e57 family=pch7 rcba=0xfed1c000 pirqg_rout=0x80 irqen=1 fn=0:29.0 pin=INTA d29ir=0x3236 -> gsi=22 via=apic-input (IOAPIC3, R68: …) == witness ::
```
and rung 2 programs input 22 active-low, level, exactly as it does for an IRQEN=0 PIRQ. The refusal arm stays behind
`IOAPIC3_APIC_INPUT` (false = flight 13's line, the go-red). Type-checked with `wc,witness,ehcihid,ioapic`.

## Why there is no QEMU reading
QEMU's ich9 router leaves IRQEN clear (`pirqa=0x0a …`, pinned in x86-default.spec), so the routed arm QEMU exercises is the one
that already existed. This arm is taken only on the rMBP.

## Boot 14 reads
1. `[ioapic] pirq … pirqg_rout=0x80 irqen=1 … -> gsi=22 via=apic-input`
2. `:: EHCI-HID: [1] ISRARM armed via=ioapic-intx gsi=22 …` in place of `REFUSED reason=pirq-disabled`
3. The EHCI completion-interrupt counters moving while keys and the pad are used — the pump no longer the only path.
If (2) is armed but (3) is flat, the next question is the redirection entry for an internal active-low source, not the PIRQ register.
