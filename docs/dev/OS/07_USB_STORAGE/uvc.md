# UVC — the USB Video Class census and the PROBE negotiation

> Subsystem doc for `unaos/crates/kernel/src/drivers/uvc.rs`. Knob `UNAOS_UVC=1` → feature `uvc`.
> Ledger row: `docs/dev/OS/rmbp-ledger.md` **B143**. Arc: CAMERA1, branch `exec-rmbp-camera1`.
> Sibling docs in this directory: `usb_xhci.md`, `xhci_hid_enumeration.md` (the EHCI HID walk this
> driver hangs off is documented in the header of `drivers/ehci/mod.rs` itself).

## 1. The device, and why it has never been touched

The 2012 15" Retina MacBook Pro's built-in FaceTime HD camera is a USB device on the EHCI bus:

```
:: EHCI-HID: [0] M1 hub-downstream device addr=2 05ac:8510 class=0xef speed=HS depth=1 (parent hub 1 port 1) tt=(hub 0 port 0) == witness ::
:: EHCI-HID: [0] addr 2 has no HID interrupt-IN endpoint — nothing to arm ::
```

(flight 11, 2026-09-22, `camera1-logs/f11.log`.) Those two lines are the complete history of this
device in this project. `bDeviceClass = 0xEF` is Miscellaneous — the class a device reports when
its interfaces are grouped into FUNCTIONS described by Interface Association Descriptors (USB 2.0
IAD ECN). It carries no HID interface, so `configure_hid` enumerated it, found nothing to arm, and
dropped it on every boot this kernel has ever taken.

The interesting number on that line is `class=0xef`, not `05ac:8510`: the gate this driver adds is
the **video IAD**, `bFunctionClass = CC_VIDEO (0x0E)` (UVC 1.1 §3.5, Table 3-1), which is a
property of the class and not of Apple. Any UVC camera on any EHCI bus takes the same path.

## 2. Scope: control transfers only, and the three things deliberately not done

| does | does not |
|---|---|
| Re-read the configuration descriptor in full on EP0 | Open an isochronous pipe |
| Walk the VideoControl interface and every VideoStreaming interface | Send `VS_COMMIT_CONTROL` |
| Record each VS alternate setting's isochronous IN endpoint | `SET_CUR` any VideoControl control |
| `GET_MIN`/`GET_MAX`/`GET_DEF`/`SET_CUR`/`GET_CUR` on `VS_PROBE_CONTROL` | Retry any transfer |

Each refusal is a decision with a reason, and each has a line on the wire:

* **No isochronous pipe.** The EHCI driver has no iTD/siTD path at all. Every transfer it issues —
  EP0 control and HID interrupt-IN alike — runs on the PERIODIC schedule, because PROBE-14 measured
  this Panther Point's async engine master-aborting its first schedule fetch in every configuration
  tried. Building an isochronous path is the NEXT rung, and it is a real piece of work, not a flag.
* **`VS_COMMIT_CONTROL` is never sent.** UVC 1.1 §4.3.1.1: a successful Commit is what arms the
  device's streaming state machine. Committing a format this kernel cannot then drain would leave
  the camera armed for a stream nobody reads, for the rest of the boot, on the same controller as
  the internal keyboard and trackpad. Wire: `[uvc] commit=withheld reason=no-iso-pipe`.
* **No `SET_CUR` on a VideoControl control.** Brightness, exposure, focus and the rest are read as
  the CAPABILITY BITMAPS the descriptors carry (UVC 1.1 Tables 3-6 and 3-8) and never written. A
  probe that changes the device is not a probe.

`SET_CUR` on `VS_PROBE_CONTROL` **is** sent, and it is not an exception to the rule above: UVC 1.1
§4.3.1.1 defines Probe as the negotiation channel — the host writes what it wants, the device
answers with what it can do — with no effect on streaming state until Commit. It is the one write
whose entire purpose is to ask a question, and `dwMaxPayloadTransferSize` in the answer is exactly
what the next rung needs in order to choose an alternate setting.

## 3. Clean room

Sources, both public USB-IF specifications, cited inline by section and table number throughout
`drivers/uvc.rs`:

* **USB Device Class Definition for Video Devices, rev. 1.1** — §3.5 (IAD), §3.7 (VideoControl
  descriptors: Tables 3-3, 3-4, 3-5, 3-6, 3-8), §3.9 (VideoStreaming: Table 3-13), §4.2/§4.3.1.1
  (class requests and the Probe/Commit block, Table 4-47/4-48), Appendix A (class, subclass,
  descriptor-subtype and request codes), Appendix B (terminal types).
* **UVC Payload: Uncompressed** and **UVC Payload: Motion-JPEG** — §3.1 (format descriptors) and
  §3.2 (frame descriptors) of each. The two frame tables are field-for-field identical, which is
  why one parser arm reads both and only the subtype decides which format the frame belongs to.
* **USB 2.0** — §9.4/§9.5/§9.6 (standard requests, descriptor concatenation, Table 9-12 interface
  and Table 9-13 endpoint), and the Interface Association Descriptor ECN.

No third-party driver source, naming or constants-by-name were consulted. Every constant in the
file is written out of a spec table and names the table it came from.

## 4. Shape

```
ehci::Controller::configure_hid
  └─ [ONE folded line, #[cfg(feature = "uvc")]]
       uvc::probe(idx, addr, cfg64, config_value, data_buf, &mut |…| self.control(t, …))
            ├─ selftest()                      once per boot, before the candidate gate
            ├─ cfg_has_video_iad(cfg64)        no traffic, no output for a non-camera
            ├─ GET_DESCRIPTOR(CONFIGURATION)   full re-read into the 256 B EP0 data buffer
            ├─ parse(full) -> Census           PURE; the whole census is its return value
            ├─ print_census()
            ├─ SET_CONFIGURATION
            ├─ GET_MIN / GET_MAX / GET_DEF     VS_PROBE_CONTROL, one transfer each
            ├─ SET_CUR(probe) / GET_CUR
            └─ "[uvc] commit=withheld reason=no-iso-pipe"
```

The driver owns no hardware, maps nothing and allocates nothing. Its only seam to the machine is
the closure — `ehci::Controller::control` with the `Target` already bound — which is what lets the
whole driver live outside the 18k-line `ehci/mod.rs` and be reached from it by one folded line.

`parse` being a pure function over a byte slice is what makes §6 possible.

## 5. The wire

⚠ READ THE PROVENANCE OF EACH LINE, because it differs. The first two lines below are a CAPTURE,
from `UNAOS_UVC=1 UNAOS_WC=1 ./arroyo test 240` on this tree (2026-09-22, rc=0, `MBENCH PASS — 6/6
required witnesses, 0 forbidden hit(s)`; QEMU's EHCI keyboard `0627:0001` at addr 1 is the device
that reaches the probe and is correctly skipped). Everything from `[uvc] candidate` down is a
SHAPE read off the source: QEMU models no UVC device, so no machine has yet produced a census or a
probe block. Flight 12 is what settles those — see the CAMERA1 row in `rmbp-queue.md` for what it
is required to print. §1's two lines are the other capture in this document.

```
:: uvc: selftest fixture=289B formats=2 frames=3 alts=2 probe_roundtrip=ok probe_reply=ok -> PASS ::
[uvc] skip addr=<n> reason=no-video-iad
[uvc] candidate addr=<n> cfg_value=<n> wTotalLength=<n> reading=<n>
[uvc] vc ctrl=<n> addr=<n> intf=<n> bcdUVC=0x0110 clock_hz=<n> vc_total_len=<n> in_collection=<n> iad first=<n> count=<n> sub=0x03
[uvc] vc term=input id=<n> type=0x0201 (camera) ctrl_len=<n> ctrl=0x<8hex>
[uvc] vc unit=processing id=<n> src=<n> ctrl_len=<n> ctrl=0x<8hex>
[uvc] vc term=output id=<n> type=0x0101 src=<n>
[uvc] vs intf=<n> ep=0x81 terminal_link=<n> formats_declared=<n> formats_seen=<n>
[uvc] vs fmt=<n> kind=<uncompressed|mjpeg|other> fourcc=YUY2 guid=<…> bpp=<n> default_frame=<n> frames=<n>/<n>
[uvc] frame fmt=<n> idx=<n> <w>x<h> default_us=<n> intervals=list n=<n>/<n> us=[…]
[uvc] frame fmt=<n> idx=<n> <w>x<h> default_us=<n> intervals=range min_us=<n> max_us=<n> step_us=<n>
[uvc] alt=<n> ep=IN<n> mps=<n> mult=<n> per_uframe=<n>
[uvc] configured addr=<n> cfg_value=<n> vs_intf=<n>
[uvc] probe stage=<min|max|def> req=0x<xx> len=<26|34> bmHint=0x<4hex> bFormatIndex=<n> bFrameIndex=<n> dwFrameInterval=<n> dwMaxVideoFrameSize=<n> dwMaxPayloadTransferSize=<n>
[uvc] probe stage=set req=0x01 len=<n> asking bFormatIndex=<n> bFrameIndex=<n> dwFrameInterval=<n> (<n> us)
[uvc] probe stage=cur req=0x81 len=<n> …
[uvc] probe negotiated asked=(f,fr,iv) got=(f,fr,iv) agreed=<true|false>
[uvc] next-rung needs alts=<n> payload_per_transfer=<n> — the alternate whose mps*mult >= that is the one to select
[uvc] commit=withheld reason=no-iso-pipe
```

**How to read it.** `agreed=` is a MEASUREMENT, not a hope: UVC 1.1 entitles a device to answer a
Probe with a different format, frame or interval than the one asked for, so "it took what we asked"
has to be a comparison printed on the line. `default_us` is `dwDefaultFrameInterval / 10` —
the spec stores frame intervals in 100 ns units, so 333333 is 33.3333 ms, i.e. 30 fps.

Failure and truncation lines, each of which is a fact and not an absence:

```
[uvc] abort addr=<n> stage=cfg-reread req=GET_DESCRIPTOR(CONFIGURATION) wLength=<n> reason=<e>
[uvc] probe stage=<s> req=0x<xx> wValue=0x0100 wIndex=<n> wLength=<n> STALLED reason=<e> — sequence ended, NOT retried
[uvc] probe skipped addr=<n> reason=<no-videostreaming-interface-in-window|no-uncompressed-format>
[uvc] census INCOMPLETE ran_short=<b> overflowed=<b> — the EP0 data buffer is 256 B …
```

## 6. The self-test, and why this rung has one

This kernel will meet exactly one UVC device — the rMBP's own camera — and only on metal. Without
a fixture, the descriptor parser would first execute on a machine nobody can single-step, against a
descriptor set nobody has read, and a wrong offset would present as a plausible-looking wrong
number rather than as a failure.

So `selftest()` drives the SAME `parse()` the wire uses over a hand-built configuration descriptor
written byte by byte out of the spec's tables, with every width, height, id and interval chosen
distinct from every other number in the fixture so that a mis-read offset cannot alias a correct
one. Its shape:

```
config (289 B) → IAD(video, 2 interfaces)
   → VC interface alt 0 { header (bcdUVC 0x0100, clock 6 MHz, wTotalLength 51),
                          camera input terminal (id 1, 0x0201, 3-byte controls bitmap 0x000C0B0A),
                          processing unit (id 2, src 1, 2-byte bitmap 0x00002D1E),
                          output terminal (id 3, 0x0101, src 2) }
   → VS interface alt 0 { input header (2 formats, ep 0x81, terminal link 3),
                          uncompressed YUY2 16 bpp + frame 1 640x480 DISCRETE (2 intervals)
                                             + frame 2 1280x720 CONTINUOUS (min/max/step),
                          MJPEG + frame 1 1920x1080 DISCRETE (1 interval) }
   → VS interface alt 1 { isochronous IN, wMaxPacketSize 0x0C00 = 1024 x 2 }
   → VS interface alt 2 { isochronous IN, wMaxPacketSize 0x1400 = 1024 x 3 }
   → VS interface alt 3 { isochronous OUT — THE NEGATIVE CASE: it must NOT enter the census }
```

Seventy-odd assertions, each naming its field. The Probe block reader is scored in the same pass
three ways: a `build_probe` → `parse_probe` round trip (a mutated offset in either is caught by the
other), a synthetic DEVICE REPLY carrying a distinct value in all six fields, and a runt block that
must be REFUSED rather than half-read.

**Three of those cases exist because a mutation sweep proved the earlier fixture could not fire**
(LAWS §5, "a check that cannot fire is an absent one"), and each is annotated in the source with
what it caught:

| what survived | why | fix |
|---|---|---|
| `parse_probe`'s `dwMaxPayloadTransferSize` moved 22 → 21 | `build_probe` leaves that field zero, so the round trip compared 0 to 0 | the `PROBE_REPLY` block, distinct value per field |
| `parse`'s packet-size mask widened `0x07FF` → `0x0FFF` | the only endpoint was `0x1400`, whose bit 11 is clear, so the wrong mask gave the right answer | alt 1 at `0x0C00`, bit 11 set |
| `f.default_frame` moved `d[6]` → `d[5]` in the MJPEG arm | `bmFlags` and `bDefaultFrameIndex` were both 1 | `bmFlags` set to 0 |
| `parse`'s `d[2] & 0x80` IN-direction test deleted | every endpoint in the fixture was already IN | alt 3's isochronous OUT, which must stay out of the census |

The self-test runs ONCE per boot, at the first device the driver is offered, **before** the
candidate gate — deliberately, because a boot with no camera on the bus is exactly the boot where
the parser is otherwise never exercised, and that is the boot the gate is for. In QEMU (which
models no UVC device) the keyboard on `ehci.0` is what reaches it.

**Go-red, and it is MEASURED — 26 mutations, 26 red, 0 survivors.** The failure arm names the field, the value
read and the value the fixture declares, and ends in the FAIL token `arroyo`'s `FAULT_PATTERNS`
makes an exit-1 for the whole run:

```
:: uvc: selftest fixture=289B failures=<n> first=<field> got=<n> want=<n> -> FAIL ::
```

Because `parse`, `parse_probe`, `build_probe`, `FIXTURE` and `selftest` are `no_std`-free of
everything but `serial_println!`, the whole section of the file ABOVE `// ── The probe ──` compiles
and runs on the host, which is how the sweep below was taken without a boot. Reproduce it in three
commands from the repo root:

```sh
SRC=unaos/crates/kernel/src/drivers/uvc.rs
python3 -c 'import sys; s=open(sys.argv[1]).read(); \
  open("/dev/stdout","w").write("#![allow(dead_code,unused_variables,unused_mut,unused_imports)]\n" \
  + s[:s.index("// ── The probe ─")].replace("serial_println!","println!") \
  + "\nfn main(){ std::process::exit(if selftest(){0}else{1}); }\n")' "$SRC" > uvc_host.rs
rustc --edition 2021 -o uvc_host uvc_host.rs && ./uvc_host; echo "rc=$?"
```

Unmutated (2026-09-22, this tree):

```
:: uvc: selftest fixture=289B formats=2 frames=3 alts=2 probe_roundtrip=ok probe_reply=ok -> PASS ::
rc=0
```

**And the same PASS reaches the WIRE**, which is the reading that counts, because it exercises the
in-kernel call path the host harness cannot: `UNAOS_UVC=1 UNAOS_WC=1 ./arroyo test 240`, rc=0,
serial.log lines 356-357 of that run —

```
:: EHCI-HID: [0] M1 root device addr=1 0627:0001 class=0x00 speed=HS -> TOPOLOGY B (direct device) == witness ::
:: uvc: selftest fixture=289B formats=2 frames=3 alts=2 probe_roundtrip=ok probe_reply=ok -> PASS ::
[uvc] skip addr=1 reason=no-video-iad
```

— the self-test ran once at the first device offered, and QEMU's EHCI keyboard was then declined
by the candidate gate at a cost of one line and zero transfers, exactly as §7 says.

**GO-RED ON THE WIRE, TOO, and it is the verb's own exit code.** With `fr.width = le16(d, 5)`
mutated to `le16(d, 6)` and nothing else changed, the SAME command returns rc=1 and arroyo's
FAULT_PATTERNS scan convicts the run by line number:

```
:: uvc: selftest fixture=289B failures=3 first=fmt0.frame0.width got=57346 want=640 -> FAIL ::
  ✖ serial.log:357: :: uvc: selftest fixture=289B failures=3 first=fmt0.frame0.width got=57346 want=640 -> FAIL ::
rc=1
```

The mutation was reverted and the file compared byte-identical to its pre-mutation copy.

Twenty-six single-edit mutations of `parse` / `parse_probe`, every one red. The sweep is
packaged with its own control probe (an unmutated run that does not PASS is a NO VERDICT, not a
green) as `camera1-logs/gates/uvc-mutation-sweep.sh`, seat-local; it ends
`uvc-sweep: red=26 survived=0`. A sample of its output, verbatim:

```
width 5->6           rc=1  … failures=3 first=fmt0.frame0.width got=57346 want=640 -> FAIL ::
ivl_type 25->24      rc=1  … failures=7 first=fmt0.frame0.interval_type got=0 want=2 -> FAIL ::
payload 22->21       rc=1  … failures=1 first=probe.reply.max_payload got=786432 want=3072 -> FAIL ::
mps mask 7FF->FFF    rc=1  … failures=1 first=alt0.mps got=3072 want=1024 -> FAIL ::
mult shift 11->10    rc=1  … failures=2 first=alt0.mult got=4 want=2 -> FAIL ::
ep dir mask drop     rc=1  … failures=1 first=alts got=3 want=2 -> FAIL ::
guid 5..21->6..22    rc=1  … failures=2 first=fmt0.fourcc got=3299669 want=844715353 -> FAIL ::
runt guard weakened  rc=101 (panic: index out of bounds — the guard is load-bearing, loudly)
```

⚠ **SCOPE OF ALL THAT EVIDENCE.** The host harness settles that the parser and the fixture agree
and that every check can fire; the QEMU run adds that the module is linked, reached from
`configure_hid`, and that `selftest` and `cfg_has_video_iad` behave in a real boot. What NOTHING
above touches is the part that needs a camera: `print_census`, the descriptor re-read, and every
`VS_PROBE_CONTROL` transfer have still never executed, because QEMU has no UVC device to offer
them. That is flight 12's job, and §8 names the first number it must print.

## 7. Bounds

Every control transfer is issued exactly once. `control()` is itself bounded (it is the same helper
the enumeration walk bounds itself with), and a failure is reported on one line naming the STAGE
and the REQUEST and then abandons the device. There is no retry loop in this file by construction:
the failure mode a camera on a shared bus can inflict is a retry storm on the controller the
internal keyboard is enumerating through, and the BTCLAIM arc's bounded-retry reasoning does not
transfer here — a camera census that misses is worth nothing and costs the keyboard.

The candidate gate runs on the 64-byte window `configure_hid` already holds and issues **zero**
transfers, so every non-camera on the bus costs one line and a walk of at most 64 bytes.

## 8. The known ceiling: 256 bytes

`control()` reads into `Controller::data_buf`, which is `qh::Buf256` — **256 bytes**, shared by
every EP0 transfer the driver makes. USB offers no way to read a configuration descriptor from an
offset (`GET_DESCRIPTOR` always returns from byte 0), so 256 bytes is a hard ceiling on the census
window, and a camera's configuration descriptor is routinely larger than that.

This is named on the wire rather than inferred:

* `[uvc] candidate … wTotalLength=<n> reading=<n> (EP0 data buffer is 256 B — the tail of this
  descriptor is UNREAD)` when the device declares more than 256.
* `[uvc] census INCOMPLETE ran_short=… overflowed=…` when the walk ran off the end of the window.
* `[uvc] probe skipped … reason=no-videostreaming-interface-in-window` when the VideoStreaming
  interface — which sits after the whole VideoControl section — did not fit.

**Flight 12 is what settles it**, and this is the number to read first: `wTotalLength=` on the
`[uvc] candidate` line. If it is ≤ 256 the census is whole and the probe runs. If it is larger, the
census is a VideoControl-only census and the next rung's first job is a bigger EP0 data buffer (or
a staged read) — one edit in `drivers/ehci/qh.rs` plus the `phys_of` alignment contract in
`ehci/mod.rs`, both outside the CAMERA1 file list and therefore deliberately not taken here.

## 9. What the next rung needs

1. **The alternate setting.** `[uvc] alt=… mps=… mult=… per_uframe=…` is the menu;
   `[uvc] next-rung needs … payload_per_transfer=<n>` from the negotiated Probe block is the
   requirement. The alternate to select is the smallest whose `mps * mult` covers it.
2. **An isochronous path in the EHCI driver.** iTDs for a high-speed endpoint, on the periodic
   schedule this driver already owns (PROBE-14 forbids the async engine on this silicon).
3. **The payload header parse.** UVC Payload Header (UVC 1.1 §2.4.3.3): `bHeaderLength`, `bmHeaderInfo`
   with the FID toggle (bit 0), EOF (bit 1) and the error bit (bit 6) — frame reassembly is the FID
   toggle and nothing else, and the error bit is what distinguishes a dropped frame from a bug.
4. **`VS_COMMIT_CONTROL`**, last, once there is something to drain the pipe.

## 10. Build

```
UNAOS_UVC=1 UNAOS_WC=1 ./arroyo test 240      # QEMU: the self-test PASS line + the skip lines
UNAOS_UVC=1 ./arroyo esp-x86                  # metal media
./arroyo knoboff uvc <parent>                 # byte identity with the knob off
```

### The knob is wired in five places, and not one of them is bookkeeping

| file | what it does | what breaks without it |
|---|---|---|
| `unaos/arroyo` knob map | `UNAOS_UVC=1` -> feature `uvc`, and the `kernel features:` banner | the knob does nothing at all |
| `unaos/builder/src/main.rs` | the x86 feature list the BOOTED kernel is built from — for `esp-x86`/`vm-image` **and for `./arroyo test`**, whose QEMU the builder owns | the banner claims the feature, the image carries none of it |
| `x86-all` + `x86-all-nowitness` legs of `KERNEL_CFG_MATRIX` | type-check the ARMED polarity of the `configure_hid` fold | the armed leg is compiled by no leg of `check` |
| `unaos/scripts/banner-cert.sh` | certifies `[uvc] commit=withheld` in the ARTIFACT | `esp-x86` exits 2, NO VERDICT, before QEMU |
| `unaos/scripts/k8-reach.registry` | records why an x86-only USB-host driver has no Pi bare-metal arm | `check` reds `k8-reach UNREGISTERED` |

The second row is the one this arc met the hard way, and it is the `sdwrite`/`rastmc` class.
MEASURED before the builder line existed, with `uvc` on the banner and absent from the artifact:

```
feature=uvc witness=[uvc] commit=withheld hits=0 -> MISSING
banner-cert: the banner and the artifact DISAGREE — this media is red.     (exit 1)
```

The gate built to catch that caught it, which doubles as the go-red proof of this feature's new
`banner-cert.sh` row — the same command on the knob-ON artifact reads `hits=1 -> OK`, exit 0. The
KNOB->BUILDER WIRING CHECK in `check_kernel_cfg` now holds the invariant from the other side: with
`uvc` named on a literal `x86-*` leg, deleting the builder line reds `./arroyo check` by name.
