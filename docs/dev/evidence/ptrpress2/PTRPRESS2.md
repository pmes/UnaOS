# PTRPRESS2 (rmbp-ledger B633): what an orphan release is on the xHCI mouse, and the fix

Flight 34, both boots, the USB mouse (vid:pid 1c4f:0034, boot-mouse on xHCI, `interval=10`):

```
[15:22:35Z] :: PTRPRESS: reports=167 press_lost=0 release_orphan=13 recovered=0 armgap_us=1013 restated=13 edges=2 held=0x00 holed=1 rearm=170 discard=0 dup=0 nobuf=0 errrearm=0 -> FAIL ::
```

## 1. What the orphan is

Source: `docs/dev/evidence/rmbp-0915/flight34/f34-boot2.log`, read with
`awk 'index($0,"PTRPRESS") || index($0,"MOUSE-1")'` and, for the drag,
`awk 'index($0,"[15:25:0") && (index($0,"MOUSE-1") || index($0,"[wm-act]") || index($0,"[clickroute]"))'`.

- **The mouse is change-only.** Between 15:23:07Z and 15:24:33Z (86 s, hand idle) the MOUSE-1 count moves
  from 1504 to 1536. A mouse that re-sent its last report at an idle rate would have made hundreds.
- **The old test was the keyboard's.** At 817f7945 `restated` meant "this report's bytes equal the previous
  report's" (`drivers/xhci/mod.rs:5048`). That is PRTSCLOST's keyboard test (`:5237`), and it is right for a
  keyboard because a keyboard report is a state. A relative mouse report is a delta. Two reports of a steady
  slow move (`[00 ff ff 00]`, then `[00 ff ff 00]`) are two real events with the same bytes. The MOUSE-1
  samples in that window are small deltas of exactly that kind (`dx=-1 dy=-1`, `dx=0 dy=-3`).
- **So the 13 orphans are 13 repeated motion reports with the button up.** No press was decoded for them
  because no press happened. A lost click was never possible here: `armgap_us=1013` is a 1 ms window, and a
  human click lasts about 100 ms. The hypothesis in the brief (a press and its release inside one re-arm gap)
  does not fit the numbers.
- **The same false positive with the button down chopped drags.** A steady drag repeats `[01 dx dy 00]`. The
  old code read that as "release lost", pushed `Button(0)`, dropped the level, and then read the same report
  as a new press. At 15:25:00Z the wire shows ten `[clickroute] press … was=0 -> drag` in one second, at
  x = 995, 989, 964, 929, 904, 898, 888, 868, 863, 858 (one leftward drag). MOUSE-1 reads `buttons=0x01` at
  reports 2464 and 2496, so the button never went up. A held button cannot make ten press edges; the
  synthesis made them. Two such re-grabs inside `DBL_MS` on a title bar give
  `zoom … route=title-dbl -> zoomed`, which is what Peter described as "changes window size like I double
  clicked". DRAGNOMOVE (B620) fixed the level half of that symptom; this is the edge half.

## 2. The fix (`unaos/crates/kernel/src/drivers/xhci/mod.rs`)

1. **The loss test depends on the report kind.** For a relative report, it counts as a no-change report only
   if it carries no motion, has no wheel, and its mask equals the decoded level. A change-only device cannot
   send such a report, so one proves an intermediate report was lost. For an absolute (tablet) report, byte
   identity is still the test. A byte-identical relative report that carries motion is counted in
   `moving_dup=` and is otherwise treated as normal. This is the motion discriminator EHCI already uses
   (ARC D M1, `drivers/ehci/mod.rs:404-412`).
2. **A click is never an orphan.** A no-change report with the mask up means a press and its release both
   happened between two decoded reports. The driver synthesises `Button(0x01)` and then `Button(0)`, in that
   order, through `pal::push_pointer_report`, so `push_locked` publishes each level (DRAGNOMOVE's seam,
   unchanged). These are counted in `click_synth=`. The press mask is 0x01 because the report shows that a
   button went down and up, not which one. A no-change report with the mask down keeps the old repair:
   synthesise the release, then read the report as a press.
3. **Four TDs on the pointer interrupt-IN.** This is XHCINTD's keyboard repair applied to the pointer. It
   uses an armed set folded onto `mouse_prev_phys`, and four 64-byte buffers carved from the existing
   512-byte `mouse_data_buffer`. A completion is retired by match, with pop-through, and anything skipped is
   counted. `mouse_prev_phys` is written when a TD is retired. Every arm site calls `mouse_top_up`. The
   completion copies the report to the stack and tops up before it decodes, so the endpoint always has at
   least N-1 TDs, including during PTRPRESS's 900 ms stall. The xHCI ring can hold several TDs; the EHCI
   overlay could not (TPSTREAM §4.3).
4. **Fixture (`tests ptrpress`).** The injected hole now swallows the first press after the stall instead of
   a release, which is the coalesced-click shape. The report after the hole belongs to the hole, so
   `restated` stays the wire's own count.

## 3. What the fold should read

QEMU (`tests ptrpress`, usb-tablet on xhci, the XHCIHUB typist's click, burst and tail):

```
:: PTRPRESS: reports=<n> press_lost=0 release_orphan=0 recovered=0 armgap_us=<g> restated=0 edges=<e> held=0x00 holed=1 rearm=<a> discard=0 dup=0 nobuf=0 errrearm=0 inflight=4 depth=4 click_synth=1 moving_dup=0 press_us=<u> skipped=0 -> PASS ::
```

`click_synth` is at least 1: the hole's swallowed press, recovered from the release that follows it at the
same tablet position. It can exceed 1 only if the wire itself restated, and then `restated` would also be
above 0 and the line would FAIL.

Metal (rmbp bench, the USB mouse): `restated=0 release_orphan=0`, with `moving_dup=` at about the old
`restated` count. A title-bar drag shows one `[clickroute] press` per gesture. On metal, `click_synth` reaches
1 only if the press the hole swallowed is released without motion. A release made while the mouse moves
cannot be told from motion, so the line FAILs `click_synth>=1` on that run. That is the fixture's limit, not
a lost click.

## 4. Bluetooth HID (read only)

`drivers/ehci/bthid.rs:1767` `ptr_deliver` is a plain level diff with no restated detector. Reports travel
over L2CAP on ACL, which is reliable and has no host re-arm window. BT HID cannot make the false synthesis
and has no window in which to lose a click. No change is owed.
