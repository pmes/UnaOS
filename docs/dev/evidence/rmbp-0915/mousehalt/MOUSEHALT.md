# MOUSEHALT: the flight-12 `xact-err-burn` retire, and why the cursor was dead

Executor MOUSEHALT, branch `exec-rmbp-mousehalt`, parent `94e90eae` (hw-rmbp), 2026-09-23. Ledgered as
rmbp-ledger **B190**. The intent is R63 ("mouse cursor and keyboard input are dead", flight 12). The brief read
the dead trackpad as the `addr=6 ep=IN1 kind=boot-mouse` halt at 32422 ms and asked for a recovery ladder for
`xact-err-burn` endpoints (M2). **M1 refutes that reading. M2 was NOT built. This is a STOP under LAWS §3 (the
behaviour on the wire diverges from the brief's premise), and the decision it needs is in §4.**

Captures read (read-only, never copied): flight 8 and flight 9 (`score89-logs/f8.log`, `f9.log`), flight 11
(`gmux7-logs/f11.log`), flight 12 (`logs/foldgate/f12-boot1.log`, 6899 lines, image 4 = `hw-rmbp@6d8d3d2d`).
All four are under `~/unaos-bench/scratch/rmbp-0915/`. The image that flew flight 12 is an ancestor of this
branch's parent, and `git diff 6d8d3d2d 94e90eae -- drivers/ehci/mod.rs` is one line, which is not in any
function cited here. So the source lines below are the code that ran.

## 1. What `xact-err-burn` means in this driver

Token `0x00048141` (addr=6) decodes (EHCI 1.0 §3.5.3) to:
- Total Bytes 4, which is `mps=4`, so nothing was transferred.
- IOC 1, PID IN.
- **CERR 0.**
- Status `0x41`, which is Halted (bit 6) plus bit 0. On a split transaction bit 0 means the TT returned an
  ERR handshake.

The controller decrements CERR once for each consecutive transaction error, starting from 3. It halts the qTD
when CERR reaches 0. Token `0x00088141` (addr=5, mps=8) has the same shape.

`halt_class` (`drivers/ehci/mod.rs:15451`) tests babble, data-buffer and missed-uframe first, and none of them
is set. Next is `tok & (ST_XACT | ST_SPLIT_ERR) != 0 || cerr == 0`, which is true on both terms, so the
function returns `("xact-err-burn", false)` at `:15473`. The `false` makes `h_clear` false at `:13859`. The
halt then goes through these steps:
1. The STOP-NOTE at `:13863` prints `-> retire`.
2. The code sets `e.dead = true`.
3. It records the retire for RETIRE-CONSEQUENCE (`:13888`).
4. `flush_held_releases` runs.

From then on, every service pass hits `if e.dead { … continue; }` (`:13694`). The QH stays linked, and nothing
ever writes it again. The driver sends no `ClearFeature(ENDPOINT_HALT)`, because the only one on this path is
KBDFLAP's (`:14327`), which runs only for `class=stall`. It does no toggle reset, because
`rearm_after_halt_clear` has one caller, the KBDFLAP stage. It does no re-arm and no port reset. The BTPROXY
block (`:15482`–`:15572`) records why the burn class was made unrecoverable on purpose: *"not one of the
bounded `ClearFeature(ENDPOINT_HALT)` budget is ever spent on them"*.

Two points on the spec, stated as facts about the code. First, `ClearFeature(ENDPOINT_HALT)` is USB 2.0
§9.4.5's answer to a device STALL. A CERR burn with a TT ERR is a transaction fault, and the device endpoint
it hit is not in the Halted feature state, so step 1 of the briefed ladder is not the spec's recovery for
this class. Second, the halt the driver sees at 32422 ms happened at some earlier time. On flight 12 the
service pass barely ran before about 33 s: `[deadman] … pmp=0` at 30–32 s, and KBDWIT reads `polls=1` at
8345 ms. So the retire time is when the pass first looked, not when the endpoint burned.

## 2. Which device addr=6 is, and where the trackpad's reports arrive

The flight-12 enumeration shows addr=6 is **not the trackpad**:

```
[   1525ms] :: EHCI-HID: [1] M1 hub-downstream device addr=4 0a5c:4500 class=0x09 speed=FS depth=2 (parent hub 3 port 1) tt=(hub 3 port 1) == witness ::
[   1775ms] :: EHCI-HID: [1] M1 hub-downstream device addr=6 05ac:820b class=0x00 speed=FS depth=3 (parent hub 4 port 2) tt=(hub 3 port 1) == witness ::
[   1799ms] :: EHCI-HID: [1] M1 hub-downstream device addr=7 05ac:8286 class=0xff speed=FS depth=3 (parent hub 4 port 3) tt=(hub 3 port 1) == witness ::
[   1829ms] :: EHCI-HID: [1] M1 hub-downstream device addr=8 05ac:0262 class=0x00 speed=FS depth=2 (parent hub 3 port 2) tt=(hub 3 port 2) == witness ::
[   5832ms] :: EHCI-HID: [1] M1 armed vendor-multitouch addr=8 ep=IN1 mps=64 interval=2 id=0x44 body=4088b (capture; hypothesis X@32 Y@34 le16, touch@46) == witness ::
```

- addr=6 is `05ac:820b`, on port 2 of the Broadcom Bluetooth hub `0a5c:4500`. The Bluetooth radio `05ac:8286`
  is on port 3 of the same hub. This is the Bluetooth HID proxy mouse named in the BTPROXY block and in the
  token table at `:14457`. It has never carried a byte.
- The trackpad is interface 1 of the internal keyboard/trackpad device `05ac:0262` at addr=8. Its endpoint is
  `ep=IN1`, `kind=vendor-mt`, and it sits behind a different TT (`hub 3 port 2`).
- On the flights where the pad was routed (8, 9 and 11), its reports arrived on addr=8 ep=IN1 as 8-byte Report
  ID 0x02 relative reports. f11: `vendor-multitouch raw report #2 (8 B): 02 00 f9 00 00 00 fb 00` at
  112433 ms. `trackpad_dispatch` (`:17436`) routes those to `TpRoute::Rel`, which prints
  `trackpad click (button-down edge …)` (`:14157`).

## 3. Population: every interrupt-endpoint halt in the four captures

| flight | addr=5 `05ac:820a` (kbd proxy) | addr=6 `05ac:820b` (mouse proxy) | recovered | trackpad `[tp] mode` | trackpad clicks (`TpRoute::Rel`) | reading |
|---|---|---|---|---|---|---|
| 8  | `tok=0x00088141 class=xact-err-burn -> retire` @ 23299 ms | `tok=0x00048141 class=xact-err-burn -> retire` @ 23299 ms | none | (not in image) | 1 | pad routed on the wire |
| 9  | same tokens, retire @ 21393 ms | same, retire @ 21393 ms | none | (not in image) | 1 | pad routed on the wire |
| 11 | same tokens, retire @ 28063 ms | same, retire @ 28063 ms | none | old write, stream stayed 0x02 | 7 | pad routed; glass: "jumping around sticky" |
| 12 | same tokens, retire @ 32422 ms | same, retire @ 32422 ms | none | **`try=legacy-index0 … latched=yes`** @ 5832 ms | **0** | glass: dead (R63) |

Across the four captures, 8 endpoints halted (2 per flight). All 8 are the two BT-proxy endpoints, and all 8
carry the same two tokens. Each one is `reports=0 clears=0/2 -> retire`, followed by `RETIRE-CONSEQUENCE …
pointer input rides addr=8 ep=IN1 only`. None recovered: `HALT-CLEAR` has 0 hits in all four captures. **No
internal-keyboard or trackpad endpoint halted in any of the four.** The addr=6 retire is identical on the
three flights whose pad reports were routed, so it cannot be what separates flight 12.

## 4. What separates flight 12: the mode switch latched and the stream changed shape

```
[   5832ms] :: EHCI-HID: [1] [tp] mode wrote=01 05 00 00 00 00 00 00 readback=01 05 00 00 00 00 00 00 latched=yes (addr=8 intf=1 widx=0 try=legacy-index0 set=true readback_ok=true) == witness ::
[ 234249ms] :: KBDWIT: [1] ep=IN1 addr=8 SILENCE-BROKE tok=0x00068d00 halted=0 … quiet_ms=210476 polls=140262 walks=140256 … reports_prior=1 toggle=1 == witness ::
[ 234250ms] :: EHCI-HID: [1] [tp] ids=02:0,44:0,other:2 sizes=2/58 first_bytes=02[] 44[] other[60 02] == witness ::
[ 234250ms] :: EHCI-HID: [1] vendor-multitouch raw report #2 (58 B): 74 57 1c 03 66 ae 03 00 00 01 07 97 1c 00 01 00 10 00 …
[ 235142ms] [deadman] up=217 hid=61 pmp=1011 hq=0 hid_ms=147 …
[ 236242ms] [deadman] up=218 hid=116 …   [ 237342ms] … hid=128 …   [ 238450ms] … hid=68 …   [ 239550ms] … hid=63 …
```

Flight 12 is the first flight on which B139's mode switch **latched**. After it, the trackpad did not go
silent. From 234 s it streamed **58-byte frames whose first byte is 0x74**, and `[deadman]` counts 436 HID
completions in 235–239 s. Over the whole boot `hid=` sums to 453 across 11 seconds: 436 trackpad and 16
keyboard, plus one more that this reading does not attribute. `trackpad_dispatch` sends any first byte other
than `0x02` or `0x44` to `TpRoute::None` (`:17448`), which counts the report in the census and delivers no
event. The capture has 0 `trackpad click` lines and 0 `trackpad format witness` lines, and no pointer event
came from the pad all boot.

The flight-12 record's sentence "`[deadman] … hid=0 … in=0/0/0` from then on" is wrong on the wire. `hid=`
is non-zero in 11 seconds after 32 s. `in=0/0/0` means the input channel accepted everything it was offered,
which is the normal reading (`deadman.rs`, the `in=` row). It does not mean no input arrived.

The keyboard was routed. addr=8 ep=IN3 printed `KBDWIT … SILENCE-BROKE` at 260696 ms, then `USB-DEBUG: KEY`
lines at 260–268 s (Tab, Tab, s, t, o, r, m, Enter). FLIGHT12.md already records that those keys reached the
desktop under the login window.

**So the dead cursor comes from a 58-byte frame format that the dispatcher does not decode. The halt played no
part.** The decision needed is a trackpad-path decision owned by B139's line. It is not an EHCI endpoint
recovery:
1. **Stop latching the vendor mode.** Keep the pad on the 8-byte id-0x02 stream that flights 8, 9 and 11
   decoded, for example by not sending the `legacy-index0` attempt. This restores flight 11's behaviour,
   including its "sticky" reading.
2. **Decode the raw frames.** The `mtraw` / `mtraw_inject` decode is knob-gated and marked "until metal proves
   raw mode is stable". Flight 12's four `raw report` lines are the first metal frames of this shape on the
   bench.
3. **Both, keyed on the readback.** Decode raw frames only when `latched=yes`, and fall back to option 1
   otherwise.

## 5. Why the briefed M2 was not built

- The only population it acts on is the two BT-proxy endpoints: 8 of 8 halts, 0 reports ever. Recovering
  them cannot move flight 13's cursor.
- It reverses a recorded decision (BTPROXY, `:15527`–`:15529`).
- It would add, on every rMBP boot, up to two `ClearFeature` control transfers and then a port reset on the
  Broadcom hub whose port 3 carries the radio that `bt-l0`… drive. That behaviour change lands on every boot
  and does nothing for R63.
- Step 1 of the ladder is §9.4.5's answer to a STALL, and this class is not a stall (§1).

The brief's own STOP clause (that CLEAR_FEATURE is impossible in the polled model) does not apply, because
the KBDFLAP path proves it is possible. The STOP here is the general one: the premise diverges from the wire.
No code changed on this branch.

## 6. What only the glass can prove

- That option 1 or 3 of §4 brings the cursor back on flight 13. The wire can show `TpRoute::Rel` traffic
  (`trackpad click`, `trackpad format witness: 8-byte id=0x02`) or decoded raw frames. Only Peter at the pad
  can say whether the arrow tracks the finger.
- Whether the 58-byte stream was also present before 234 s with no finger on the pad. The census rolls up only
  when `EHCIDARK` moves, and `hid=0` from 33 s to 234 s says nothing arrived, so the reading is that Peter first
  touched the pad at 234 s. This is `unverified` until he says so.
