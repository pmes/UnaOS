# TPSTREAM (rmbp-ledger B624): the trackpad's transfer path, audited against the wire

Peter, flight 34: "desktop up mouse highly responsive!!!!!", then "ok, nevermind. that test went horribly" (the
trackpad). On 2026-10-14 he asked: "is trackpad code chicken wired in? why is it doing so bad?"

## 1. The wire (docs/dev/evidence/rmbp-0915/flight34/f34-boot1.log)

```
[15:00:59Z] :: EHCI-HID: [1] [tp] mode req=1 rt=0xa1/0x01 addr=9 if=1 widx=0 wValue=0x0300 status=ok got=8b ms=1 ::
[15:00:59Z] :: EHCI-HID: [1] [tp] mode req=2 rt=0x21/0x09 addr=9 if=1 widx=0 wValue=0x0300 status=ok got=8b ms=0 ::
[15:00:59Z] :: EHCI-HID: [1] [tp] mode req=3 rt=0xa1/0x01 addr=9 if=1 widx=0 wValue=0x0300 status=ok got=8b ms=0 ::
[15:01:25Z] :: EHCI-HID: [1] [tp] mode-mismatch readback=vendor stream=legacy -> route=legacy (the wire beats the register) == witness ::
[15:01:25Z] [tp] mt route=legacy ep=1 buttons=0x00 dx=0 dy=-2 ids=02:1,44:0,other:1 (HIDSTALL2: one line per 10 s while a finger is down)
[15:01:25Z] [tp] mode reassert n=1 why=stream-legacy addr=9 if=1 was=0x01 normal=ok vendor=ok readback=0x01 readback_ok=true latched=yes ms=11 (…)
[15:02:08Z] [tp] mt route=vendor ep=1 fingers=1 mover=0 x=-1818 y=2372 dx=0 dy=0 buttons=0x00 frames=497 ids=02:1,44:0,other:498 (…)
[15:02:13Z] :: POINTERFEEL: reports=133 rate_hz=26.6 gaps=3 gap_max_ms=1447 worst_holder=silent/pass_max_ms=5 route=mt scale=5.55u/px range=7994 carried=0/142 carried_pct=0 expected_hz=125 poll_hz=500 binterval=2 speed=fs rate_pct=21 rate=LOW -> FAIL :: … mt=133 legacy=0 reassert=1 reassert_latched=1 …
[15:02:13Z] [ptrfeel] gap n=1 at_ms=979 ms=578 holder=unheld/step=- (…) pass_max_ms=5 cause=silent
[15:02:13Z] [ptrfeel] gap n=2 at_ms=3034 ms=1447 holder=unheld/step=xhci (…) pass_max_ms=5 cause=silent
[15:02:19Z] [tp] mt route=vendor ep=1 fingers=1 mover=0 x=1829 y=5549 dx=0 dy=0 buttons=0x00 frames=928 ids=02:1,44:0,other:929 (…)
```

The hid pass ran at most 5 ms apart over every hole (`pass_max_ms=5`, `holder=unheld`), so no kernel hold explains the
holes. The boot carries no `ISRARM` or `EHCIDARK` line (both are census-gated), so the wire could not say which path
retired each frame.

## 2. The path as found (unaos/crates/kernel/src/drivers/ehci/mod.rs at d616bf98)

| piece | as found | where |
|---|---|---|
| QH endpoint chars | addr 9, EP1, EPS=full, DTC=1, MPS=64 | `arm_interrupt_ep`, :13472–13476 |
| S-mask | 0x01: the start-split goes out in µframe 0 of every frame | :13484 |
| C-mask | 0x1C: complete-splits in µframes 2, 3, 4. This is the Y+2..Y+4 window for a start in Y=0 (EHCI 1.0 §4.12.2, USB 2.0 §11.18.4); a 64 B FS packet takes about 50 µs on the downstream bus | :13477–13482 |
| TT fields | hub address and port from the enumeration `Target` (the root hub's TT) | :13477–13482 |
| FSTN | none, and none is needed: a start in µframe 0 never wraps a frame | — |
| poll rate | the QH is linked from all 1024 frame-list entries, so one poll per 1 ms frame. This over-serves bInterval=2, which the spec allows | :13508–13514 |
| depth | ONE transfer, held in the QH overlay (overlay-direct). Both rMBP EHCI functions raise HSE on a qTD fetch, so the controller is never given a qTD to chain | :13489–13497; ISRARM "Why depth was refused" |
| armed size | `rx_total` = `INT_BUF_LEN` = 256 B for the vendor endpoint (TRACKPADPANE 963dd2ff); the transfer retires on the frame's short packet | :13464 |
| completion seen by | (a) the ISR (USBINT on IOC → `interrupt_ack` → `isr_service_ep`): copies the frame into the hand-off ring and re-arms; (b) the hid pass `service` (about 1 kHz), which reads the overlay token. The first to arrive takes it | :15126; :13813–13945 |
| interrupt latency | USBCMD.ITC is never written, so it keeps its reset value of 8 µframes (1 ms); the controller raises USBINT up to 1 ms after a completion | init |

## 3. The defect

In the pass's arm the frame length was `mps - residue` (`:13943`). When 963dd2ff raised the armed total from 64 to 256,
this line was left on `mps`. A one-finger TYPE2 frame is 58 B, which leaves a residue of 198, and `64 - 198` saturates
to **0**. A two-finger frame (86 B, residue 170) also reads 0. So every frame that the pass retired before the
interrupt did was read as empty, skipped by `if len > 0`, and re-armed. The frame was gone, and no counter, line or
holder recorded it. The ISR's arm computed the length correctly from `rx_total`.

So the pad delivered only the frames the interrupt reached before the pass did. Two things gave the pass a large share:

- ITC held the interrupt for up to 1 ms.
- The pass polls about every millisecond.

Any stretch where the MSI's core was slow to take the vector gave every frame to the pass. That matches POINTERFEEL's
reading exactly: a `silent` gap with the pass on cadence, and 21 % of the pad's rate.

`mover=0 dx=0 dy=0` on the sampled `[tp] mt` rows is not a dropped frame. `mover` is the index of the finger that drives
the pointer (0 = finger[0]; `TpCensus::mt_step`). `dx`/`dy` are the whole pixels output after the sub-pixel carry. A
finger-down frame with no motion is still counted (`pointerfeel::note(now_ms, true, f.fingers > 0)`).

**The mode.** The pad's FIRST report on that boot was legacy (`ids=02:1`, at the first touch, 15:01:25Z, after the
password was typed). POINTERFEEL's re-assert was triggered by that report on the wire and ran 11 ms later. Every report
after it was vendor (`mt=133 legacy=0`; `other:498 → 929`). One report was lost to the mode, not a minute of them: the
stream "stayed legacy" only until the pad first had something to say.

## 4. The fix

1. **The length** (`:13943`): `rx_total - residue`, with `rx_total` read from the ISR registry, which is the same field
   the re-arm arms with. This closes the loss: a frame the pass retires is now decoded, routed and counted.
2. **ITC = 1 µframe**: written at the two post-HCRESET run writes in `init` (`:18628`, `:18872`), where HCHalted is 1.
   EHCI §2.3.1 leaves ITC undefined if it is written while the controller runs, so a controller that was not reset keeps
   the firmware's value, and the `[tp] stream` row prints `itc=` to show which case applies. With ITC at 1 µframe the
   re-arm follows the completion by at most 125 µs.
3. **The ring the brief asked for is refused.** A qTD ring cannot be built on this silicon (overlay-direct, HSE on a
   qTD fetch: ISRARM; POINTERSCALE2 B554 refused it for the same reason). The one transfer is re-armed from the
   completion interrupt. A device whose poll finds the endpoint re-arming keeps its frame for the next IN; the NAK is
   the device's, not a loss.
4. **The instrument**: one `[tp] stream` row beside `[tp] mt`, at the same one-line-per-10-s gate while a finger is
   down: `[tp] stream ep=<c> frames=<n> isr=<n> poll=<n> rescued=<n> nak=<n> err=<n> csplit_miss=<n> rearm=<n>
   inflight=1 depth=overlay itc=<n>uf smask=0x01 cmask=0x1c tt=hub<a>:port<p> mps=64 irq=<n>`. `nak` counts passes that
   found the transfer still active, because EHCI keeps no NAK counter for a periodic QH. `err` and `csplit_miss` are
   the token's XactErr and Missed-Micro-Frame bits. `rescued` counts pass completions that the old line would have
   read as 0.
5. **The mode by the stream**: `:: EHCI-HID: [1] [tp] mode ok stream=vendor at_ms=<n> try=<n> readback=vendor …` is
   printed on the first vendor frame since boot (try=0) and on the first after each re-assert (try=n). The re-assert
   stays bounded (`pointerfeel::REASSERT_MAX` = 2, 2 s apart).
6. **The fixture** (`tests pointerfeel`): the verdict line gains `nak=<n> err=<n> csplit_miss=<n> via_isr=<n>
   via_poll=<n> rescued=<n> inflight=1`, counted inside the window.

## 5. What the bench should read after the fix

With a moving finger in `tests pointerfeel`:
`:: POINTERFEEL: reports=<≥500> … gaps=0 … expected_hz=125 … rate_pct=<≥80> rate=ok -> PASS :: … nak=<n> err=0
csplit_miss=0 via_isr=<n> via_poll=<n> rescued=<n> inflight=1`. `via_isr + via_poll` should be about `reports`, and
`rescued` should be about `via_poll`: before the fix, those are the frames that were lost. If the rate stays LOW with
`via_poll` small and `nak` large, the pad itself sent fewer frames, and the next suspect is the TT budget
(`csplit_miss`).

## 6. Peter's question

What the code does today: the pad's frames reach the kernel two ways, through the completion interrupt or through the
hid pass, whichever sees them first. What was missing: when the trackpad's buffer was enlarged to hold multi-finger
frames, the hid pass's length calculation was left on the old 64-byte size. Every frame the pass saw first was read as
empty and thrown away, without a line or a counter. The fix computes the length from the size the transfer was armed
with, makes the interrupt fire within 125 µs instead of up to 1 ms, and puts both paths' counts on the wire.
