# IOAPIC2 — the ISRARM reason and the chipset PIRQ route (rmbp-ledger B191)

Branch `exec-rmbp-ioapic2`, cut at `94e90eae` (hw-rmbp). M1 `558931c4`, M2 `2eb57454`. Every
capture below is a QEMU q35 boot of this branch; the metal half is flight 13. Mechanism:
`docs/dev/OS/01_BOOT_HAL/ioapic.md` §9.

## The defect (flight 12, `f12-boot1.log`, seat-local slice, read-only)

```
[   5833ms] [ioapic] route bdf=0:29.0 pin=INTA line=0 -> REFUSED reason=no-firmware-line — …
[   5833ms] :: EHCI-HID: [1] ISRARM REFUSED — this function offers no usable MSI capability, and
            there is no IOAPIC in this kernel to route INTx to. …
[  32419ms] :: BPACE: ehci-hid-done t=5833ms d=5538ms ::
```

## M1 — the refusal names its reason (`558931c4`)

`m1-knoboff-serial.log` — `UNAOS_WC=1 UNAOS_QEMU_FULL=1 ./arroyo test 120`, rc=0,
`x86-default.spec` 19/19 `[full wall 121.9s]`, sidecar `completion=complete`:

```
:: EHCI-HID: [0] ISRARM REFUSED reason=no-ioapic-in-kernel — MSI: this function offers no usable
   MSI capability; INTx: the I/O APIC route was refused for the reason named (its own `[ioapic]`
   line says why). The endpoint stays on the POLLED re-arm path, … == witness ::
```

The first run of that lane was rc=1 on `:: SERWIT-2: FAIL — balanced=true evidence_lost=5 ::` at
load average 36.65 (FIXTURE_FLAKES §2a, the known flight-recorder flake); re-run alone, green.
Control before any run: the two new rows replayed against a pre-change capture
(`rmbp-0923/dockvac/g1-wc-serial.log`) — FORBID hit at line 420, REQUIRE 0 hits.
GO-RED (source mutation, reverted): the stale sentence restored in the format string, diff sha256
`bf5ab7f1f933c4fb5aaafd250aa48ea68d93bc42dc736fa889bf301ea822f758` — `❌ FORBID there is no IOAPIC
in this kernel to route INTx to` (hit at line 443), REQUIRE 0 hits, `MBENCH FAIL — 18/19`, rc=1;
restored `drivers/ehci/mod.rs` sha256 `1b9bdd21b6501e49b98626f03e92618a8e38adafbcbd61e955c168516246a7f1`
= the pre-mutation hash.

## M2 — the chipset PIRQ route (`2eb57454`)

`m2-pirq-serial.log` — `UNAOS_WC=1 UNAOS_IOAPIC=1 UNAOS_QEMU_FULL=1
UNAOS_QEMU_EXTRA="-qmp tcp:127.0.0.1:4489,server,nowait" ./arroyo test 150`, beside
`scripts/qmp_type.py --port 4489 --connect-timeout 600 --marker ':: EHCI-HID: [0] M2 armed keyboard'
--marker-log target/serial.log --marker-timeout 300 --wait 3 --bursts 6 --burst 9`
(`m2-pirq-typist.txt`: 120 events). rc=0, 19/19, `[full wall 152.0s]`, sidecar
`completion=complete`, banner `⚡ kernel features: witness,ehcihid,kbdwit,sdw,sdhcblk,smolnet,wc,sdwrite,ioapic`:

```
:: EHCI-CONFIG: [0] bdf 0:29.0 id 8086:24cd — begin wake sequence ::
[ioapic] pirq bdf=0:31.0 id=8086:2918 family=ich9 rcba=0xfed1c000 pirqa=0x0a pirqb=0x0a pirqc=0x0b pirqd=0x0b pirqe=0x0a pirqf=0x0a pirqg=0x0b pirqh=0x0b fn=0:29.0 pin=INTD d29ir=0x3210 -> pirq=D gsi=19 line=11 fw_line=11 fw_agree=yes == witness ::
[ioapic] route bdf=0:29.0 pin=INTD line=11 -> gsi=19 via=pirq polarity=active-low trigger=level == witness ::
[ioapic] armed bdf=0:29.0 gsi=19 vector=0x43 masked=false dest_apic=0 entry=0x000000000001a043 unmasked_lo=0x0000a043 intx_disable=0 routed=1 == witness ::
:: EHCI-HID: [0] ISRARM armed via=ioapic-intx gsi=19 vector 0x43, USBINTR 0x00000000 -> 0x00000001 (USBINT unmasked). …
:: EHCI-HID: ISRARM armed=1 refused=0 irq=122 isr_rearm=114 poll_rearm=8 depth_max=8 ringfull=8 cont_isr=0 cont_poll=0 oversize=0 == witness ::
```

GO-RED (source mutation, reverted): the wrong PIRQ index (`idx + 1`), diff sha256
`41f88f0522e3b0473bd1d3bd79591aa68ddd628c18383e796cb20b455023f2cd`, same lane and typist
(`m2-gored-serial.log`), rc=1:

```
[ioapic] pirq … fn=0:29.0 pin=INTD d29ir=0x3210 -> pirq=E gsi=20 line=10 fw_line=11 fw_agree=no == witness ::
[ioapic] armed bdf=0:29.0 gsi=20 vector=0x43 masked=false … unmasked_lo=0x0000a043 …
:: EHCI-HID: ISRARM IRQ DEAD — completion interrupt armed (MSI or I/O APIC INTx) on 1 controller(s) … (irq=0 isr_rearm=0 poll_rearm=0) …
```

`❌ FORBID \[ioapic\] pirq .* fw_agree=no`, `MBENCH FAIL — 19/19 required witnesses, 1 forbidden
hit(s)`; `ISRARM armed=` rollups: 0 (4 on the pass). Restored `arch/x86_64/ioapic.rs` sha256
`3b3e039b3693b2b14274fcb841f044c97751ee1bca1f2353af874a660b37b299` = the pre-mutation hash.

⚠ On EVERY routed pass the dead verdict printed too, before the typist's first key (`m2-pirq`:
`… NOT been delivered once in 5011 ms (irq=0 …)`, then `irq=122`). `ISRARM IRQ DEAD` fires on
uptime ≥ 5 s with zero deliveries, and a NAKing interrupt-IN endpoint completes nothing, so a
correct route on a quiet bus reads DEAD. Flight 13 must read it beside the rollup.

## M3 — `ehci-hid-done`, six runs on `2eb57454` (clean tree, sidecar `sha=2eb57454`)

`isr_arm_controller` runs at the END of `ehci::init`, and `ehci-hid-done` is stamped when `init`
returns (`arch/x86_64/pci.rs:851`), so the interrupt cannot shorten `d=`. Measured anyway, fast
mode (completion + 20 s grace), all rc=0, `x86-default.spec` 19/19; load average 14-18.
BEFORE = polled, `UNAOS_WC=1 ./arroyo test 120`; AFTER = routed,
`UNAOS_WC=1 UNAOS_IOAPIC=1 UNAOS_QEMU_EXTRA=… ./arroyo test 120` + the typist above.

| run | `ehci-hid-done` | ISRARM | rollup |
|---|---|---|---|
| before 1 | `t=3118ms d=290ms` | `REFUSED reason=no-ioapic-in-kernel` | — |
| before 2 | `t=2896ms d=316ms` | `REFUSED reason=no-ioapic-in-kernel` | — |
| before 3 | `t=3532ms d=336ms` | `REFUSED reason=no-ioapic-in-kernel` | — |
| after 1 | `t=4109ms d=338ms` | `armed via=ioapic-intx gsi=19` | `irq=122 isr_rearm=114 poll_rearm=8` |
| after 2 | `t=3101ms d=321ms` | `armed via=ioapic-intx gsi=19` | `irq=122 isr_rearm=114 poll_rearm=8` |
| after 3 | `t=3951ms d=346ms` | `armed via=ioapic-intx gsi=19` | `irq=120 isr_rearm=112 poll_rearm=10` |

Means 314 ms polled, 335 ms routed: no drop, inside the load spread. The brief's M3 prediction
("`ehci-hid-done` drops") is refuted on this tree by construction and by measurement. What the
route buys is the re-arm: 112-114 of 120-122 endpoint re-arms from the ISR on every routed run.
The wire shows which path is live on every run (`REFUSED reason=…` vs `armed via=ioapic-intx gsi=19`).
