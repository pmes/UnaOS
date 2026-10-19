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

## 11. XHCIISO (rmbp-ledger B534) — the stream on xHCI

REALSENSE (SR92) found that no camera on this machine had ever delivered a frame: §2's withheld commit
on EHCI, no isochronous path anywhere, and the D435i's native bus is xHCI. This arc builds the stream
on **xHCI** and leaves EHCI exactly as §2–§8 describe it (its `commit=withheld reason=no-iso-pipe`
is still true there, and `banner-cert.sh` still certifies it). Code: `drivers/xhci/isoch.rs` (new,
cfg-erased with the knob) and the `XHCIISO` section of `drivers/uvc.rs` between `probe` and the
fixture; `drivers/xhci/mod.rs` gains eight same-line cfg-gated folds and no lines. No new knob:
everything is under `uvc`, and on x86_64 only (`all(target_arch = "x86_64", feature = "uvc")`, the
`pub mod uvc` gate).

```
root FSM "no driver for class 0xef"  ─┐ isoch::note_root        (no traffic: a queue of 4 slots)
hub walk, video IAD in the 64 B window ┘ isoch::note_downstream
service_ftdi ladder slot → service_uvc (never while a root port enumerates; one candidate a pass)
  └ 64 B window into the 4 KiB EP0 buffer → uvc::probe_iso(.., &mut XhciUvc)
       full cfg re-read (≤ 4096) → parse → census → first VideoStreaming function (cut)
       SET_CONFIGURATION → GET_MIN/MAX/DEF → SET_CUR/GET_CUR(Probe)
       alternate = smallest whose ESIT bytes ≥ dwMaxPayloadTransferSize
       Configure Endpoint (Add, Isoch IN) → SET_CUR(Commit) → SET_INTERFACE(alt) → ring fill + doorbell
       "[uvc] commit=armed pipe=iso ep=<n> mps=<n> mult=<n> interval=<n> …"
transfer event (slot, DCI) → isoch::claim: retire TDs in ring order through the named TRB,
       uvc::stream_payload(TD bytes) → Assembler (FID/EOF/ERR, double buffer) → latest_frame()
       re-produce one TD per retired one, Frame ID from MFINDEX, doorbell
```

### 11.1 Capture

None yet (DRIVERS-METHOD §2). The first flight's `[uvc-iso] cfg-ep`, `cfg-ep out`, `start` and the
first four `[uvc-iso] event` lines ARE the capture: the endpoint context as the controller holds it
after Configure Endpoint (DW0/DW1/DW4 decoded), MFINDEX at the start, and the first completions'
codes and residuals. Kept, when it exists, in `docs/dev/evidence/<round>/xhciiso/`.

### 11.2 Rung ledger

No rung below has a `STATUS.tsv` row yet; each is `open` until the fold's flight quotes its line.

| rung | hypothesis | writes | confirms | refutes | alternatives |
|---|---|---|---|---|---|
| R0 premise | no isochronous pipe exists on any host here, so no frame ever arrived | none | REALSENSE's `commit=withheld reason=no-iso-pipe` on every boot (EHCI) and no xHCI UVC path in the tree (read) | a `[uvc]` frame count from an earlier boot (none known) | — |
| R1 EP0 > 256 B | xHCI EP0 needs only a bigger buffer: one Data TRB of ≤ 4096 B, 4 KiB-aligned | GET_DESCRIPTOR(CONFIGURATION, wTotalLength ≤ 4096) | `[uvc] candidate … host=xhci … reading=<wTotalLength>` and a census without `INCOMPLETE` | `[uvc] abort … stage=cfg-reread reason=<e>` | a controller that splits the Data stage (none known) |
| R2 Configure Endpoint | the Isoch IN context (§6.2.3: EP Type 5, CErr 0, MaxBurst, Mult, Interval, Max ESIT Payload) is accepted | Input Context A0+A(dci), Configure Endpoint | `[uvc-iso] cfg-ep … epstate=0(Disabled)->1(Running) result=cc=1 (Success)` | `result=Bandwidth-Error` / `Parameter-Error` / `Resource-Error`, and `[uvc] commit=withheld reason=configure-endpoint-refused cc=<n>` | a wrong Interval encoding (refuted by `cfg-ep out dw0 interval=`), Max ESIT Payload split (dw0 esit_hi / dw4 esit_lo) |
| R3 commit + alt | SET_CUR(Commit) with GET_CUR's block, then SET_INTERFACE(alt) | the two control requests | `[uvc] commit=armed pipe=iso …` | `[uvc] commit=failed stage=commit\|set-interface reason=<e>` | a device that wants SET_INTERFACE before Commit (Linux's order is Commit first) |
| R4 Frame-ID scheduling | TDs with Frame ID = (MFINDEX + IST + 16 µframes) >> 3, then one interval apart, are executed | Isoch TRBs (§6.4.1.3), doorbell DB[slot]=DCI | first `[uvc-iso] event` cc=1 or 13 with `residual` < td_len | 64 missed with 0 delivered → `[uvc-iso] sched frameid->sia …` (the SIA rung runs in the same boot) | SIA (bit 31) from the first TD; a Frame ID off by the IST unit (frames vs µframes, §5.3.4) |
| R5 TBC/TLBPC | HS: TBC 0, TLBPC = packets−1; SS: bursts of bMaxBurst+1 [ONE-SOURCE + recalled Linux] | same TRBs | cc=1/13 events | cc=5 TRB Error on the first TD | the HS mult encoded into TBC instead |
| R6 payload/frames | the UVC payload header (§2.4.3.3) delimits frames by FID toggle/EOF | none (host only) | `tests uvc` → `frames_2s>0`, `good>0` | `payloads>0 frames_2s=0` (header/FID reading wrong), `bad_hdr` ≈ `payloads` | a camera that streams MJPEG only (the format pick takes the first UNCOMPRESSED one) |

### 11.3 This boot's tree

Read top to bottom; the first line that differs from its "confirms" column is the wall.

1. no `[uvc-iso] candidate` → the camera did not reach either fold (root: its device class is not
   0xEF; hub: no video IAD in the first 64 bytes) — read the `xHCI: no driver for device class` line.
2. `[uvc] candidate … host=xhci` then `[uvc] abort` → R1.
3. `[uvc-iso] cfg-ep … result=<not Success>` → R2; the `cfg-ep out` line names the field.
4. `[uvc] commit=failed` → R3.
5. `[uvc-iso] start …` and then no `[uvc-iso] event` at all → the doorbell or the context's TR
   Dequeue Pointer (read `cfg-ep out`), or a Frame ID the controller holds for later (compare
   `mfindex=` / `first_frame~=`).
6. events with cc=23 only → R4 refuted; the `frameid->sia` line follows within 64 TDs and the SIA
   rung decides in the same boot.
7. events cc=5 → R5. Events cc=1/13 but `frames_2s=0` → R6.

### 11.4 Walls known and unapplied

* **Bulk-streaming cameras.** A UVC function whose VideoStreaming alt 0 carries a BULK IN endpoint
  (UVC 1.5 §2.4.3: bulk is allowed) gets `commit=withheld reason=no-iso-in-alternate`; whether the
  D435i streams isochronous or bulk on this bus is unread (REALSENSE's census line names its
  endpoints — read it first).
* **The D435i's three functions.** `probe_iso` streams the FIRST VideoStreaming function only (the
  cut, `first_function_len`); REALSENSE's `multi::census` names which role that is. REALSENSE's two
  `sonde` folds live in EHCI's `probe`; on xHCI the same two calls belong in `probe_iso` (after the
  full re-read and after `parse`) — a seat fold, both branches being unfolded.
* **One stream per boot** (`one-stream-already-armed`), **no re-arm after a detach** without a
  re-enumeration, and the ring/TD buffers of a detached stream are leaked (≤ 1 MiB + 4 KiB each):
  the controller may still hold the TR Dequeue Pointer until the slot is disabled.
* **A camera with a HID button** behind a hub is armed by the HID walk first (its SET_CONFIGURATION
  is the same value); `probe_iso`'s SET_CONFIGURATION repeats it and resets the HID endpoint's
  toggle on the device side. Unseen; named.
* **SuperSpeedPlus isochronous companions** (USB 3.2 §9.6.8) and ESITs over 64 KiB are refused by
  name (`esit-over-64KiB`).

### 11.5 Constants

| constant | value | sources |
|---|---|---|
| Isoch TRB type / fields | 5; ISP b2, IOC b5, TBC b8:7, BEI b9, TLBPC b19:16, Frame ID b30:20, SIA b31 | xHCI 1.2 §6.4.1.3 · Linux xhci-ring.c (recalled) |
| Endpoint Context Isoch IN | EP Type 5, CErr 0, Mult DW0 9:8 (SS only), Interval DW0 23:16, Max ESIT Payload hi DW0 31:24 / lo DW4 31:16 | xHCI 1.2 §6.2.3 · the hub path's DW0/DW4 fix in mod.rs (XHCIHUB S1) |
| Interval encoding | HS/SS bInterval−1; FS bInterval−1+3 | xHCI 1.2 §6.2.3.6 · USB 2.0 Table 9-13 · the hub path's encoding |
| MFINDEX / IST | Runtime+0 b13:0; HCSPARAMS2 b3:0 (b3 = frames) | xHCI 1.2 §5.5.1, §5.3.4 [ONE-SOURCE] |
| Frame window | > MFINDEX+IST, < +895 ms; start margin 16 µframes (chosen) | xHCI 1.2 §4.11.2.5 [ONE-SOURCE] |
| Ring slack 8 | spurious Success after Short Packet (Panther Point 0x1e31) | Linux XHCI_SPURIOUS_SUCCESS · mod.rs `mouse_prev_phys` |
| TBC/TLBPC | HS 0 / packets−1; SS ceil(p/(B+1))−1 / last burst−1 | xHCI 1.2 §6.4.1.3 · Linux xhci_get_burst_count (recalled) |
| Payload header | bHeaderLength, bmHeaderInfo FID b0 EOF b1 PTS b2 SCR b3 STI b5 ERR b6 EOH b7; PTS 4 B, SCR 6 B | UVC 1.5 §2.4.3.3 (= 1.1) · the self-test |
| VS_COMMIT_CONTROL | 0x02 | UVC 1.1 A.9.8 |
| SS companion | type 0x30: bMaxBurst[2] bmAttributes[3] (Mult b1:0) wBytesPerInterval[4..6] | USB 3.2 §9.6.7 Table 9-27 · the self-test's SS fixture |

### 11.6 Unwind

* Configure Endpoint refused → nothing on the device changed (no Commit yet); the ring and TD
  buffers are freed (never referenced by a context that took effect).
* Commit or SET_INTERFACE failed → Configure Endpoint with the Drop flag; the device is left at alt 0.
* Ring start failed → SET_INTERFACE(alt 0), Drop, the assembler disarmed.
* An early return after a successful Configure Endpoint → `service_uvc` drops the endpoint.
* Detach → `isoch::disconnect` unpublishes the pipe and disarms the assembler (buffers leaked, above).
* Nothing here is destructive: every write is a standard request or a host-controller command, and
  alt 0 returns the camera to its idle state (USB 2.0 §9.4.10, UVC 1.1 §2.4.3).

### 11.7 What the next flight reads

`UNAOS_UVC=1` media, a UVC webcam on an xHCI port (and the D435i on another boot), then
`tests uvc`:

```
[uvc-iso] candidate slot=<n> path=<root|hub> queued
[uvc] candidate addr=<slot> host=xhci cfg_value=<n> wTotalLength=<n> reading=<n> ep0_cap=4096
[uvc] … census lines (§5) …
[uvc] probe negotiated host=xhci asked=(f,fr,iv) got=(f,fr,iv) agreed=<b> dwMaxVideoFrameSize=<n> dwMaxPayloadTransferSize=<n>
[uvc] alt-choice alt=<n> ep=0x8<n> esit=<n> need=<n> why=<smallest-covering|widest(…)> ss=<b> burst=<n> mult=<n> bpi=<n>
[uvc-iso] cfg-ep slot=<n> dci=<n> speed=<n> type=isoch-in mps=<n> burst=<n> mult=<n> esit=<n> interval_enc=<n> (<n> uframes) ring=0x… slots=<n> epstate=0(Disabled)->1(Running) result=cc=1 (Success)
[uvc-iso] cfg-ep out dw0=0x…[state=1 mult=<n> interval=<n> esit_hi=<n>] dw1=0x…[type=5 burst=<n> mps=<n>] dw4=0x…[avg=<n> esit_lo=<n>]
[uvc-iso] start slot=<n> dci=<n> tds=<n> slack=8 stride=<n> td_len=<n> interval_uf=<n> ist_uf=<n> mfindex=<n> first_frame~<n> lead_uframes=<n> sched=frameid ring=0x… bufs=0x…
[uvc] commit=armed pipe=iso ep=<n> mps=<n> mult=<n> interval=<n> alt=<n> esit=<n> payload=<n> host=xhci tds=<n> lead_uframes=<n> sched=frameid frame=<w>x<h> fourcc=YUY2 frame_bytes=<n> store=double-buffer
[uvc-iso] event n=0 slot=<n> dci=<n> cc=<13 (Short-Packet)|1 (Success)> trb=0x… residual=<n> mfindex=<n> outstanding=<n>
:: uvc: selftest fixture=289B formats=2 frames=3 alts=2 probe_roundtrip=ok probe_reply=ok -> PASS ::
:: uvc: stream-selftest hdr=ok asm frames=4 good=1 short=1 errf=1 over=1 ep=ok ss=ok cut=ok -> PASS ::
:: UVC: selftest=PASS stream_selftest=PASS host=xhci addr=<n> <w>x<h> fourcc=YUY2 frames_2s=<n> fps=<f> good=<n> short=<n> err=<n> over=<n> payloads=<n> bytes=<n> bad_hdr=<n> tds=<n> missed=<n> overrun=<n> xact=<n> dup=<n> resync=<n> sched=<frameid|sia> -> PASS ::
```

QEMU (no UVC device on its xHCI unless a host webcam is passed through) reads, by name:

```
:: UVC: selftest=PASS stream_selftest=PASS host=- frames_2s=- fps=- -> SKIP reason=qemu-no-uvc-stream ::
```

and metal with no camera on xHCI reads `-> SKIP reason=no-xhci-uvc-stream`. A FAIL names the half
that failed (`selftest=FAIL` / `stream_selftest=FAIL`, the first failing field on its own line), or
`frames_2s=0 … -> FAIL reason=no-frames` with the counters that say where the bytes stopped.
`tests sonde`'s `frames_2s>0` (the row's last item) is REALSENSE's fixture and reads this stream
only once the seat folds the two branches.

## 12. UVCCAM (rmbp-ledger B686, R130) — the picture: frames to a window

R130's second law (no paternalism): Peter's Sony mirrorless shows live view on no Apple machine. The class
driver (§1–§10, CAMERA1 B143) and the stream (§11, XHCIISO B534) end at `uvc::latest_frame`, the assembler's
front buffer, and before this arc **nothing read it** — no frame of any camera reached the glass. UVCCAM is
that consumer. Code: `drivers/xhci/uvc.rs` (new, `CHARTER: Stria — driver`, declared `#[cfg(feature = "uvc")]`
on `xhci/mod.rs`'s module line); `xhci/mod.rs` gains four same-line folds and no lines (the module, `note_dev`
on the root "no driver for class" arm and on the hub walk, `pump` beside `service_uvc` on the ladder);
`tests.rs` one same-line fold (`tests uvccam`). No new knob: everything is under `uvc`.

```
probe_iso (drivers/uvc.rs)        ── census(slot, &Census) ─→ "[uvc] dev=<vid:pid> formats=<n> mjpeg=<w>x<h>@<fps> yuy2=… frames=<n>"
  stream_arm(w, h, fourcc)        ── arm(slot, w, h, cc)       (atomics; GENERATION += 1)
  stream_disarm (unwind, unplug)  ── disarm()
service_ftdi ladder → service_uvc → uvc::pump()  — starts ONE `uvc-view` task per armed stream (x86 `wc`)
uvc-view, every 8 ms while armed:
  latest_frame → memcpy OUT under the stream lock (never an allocation there: the claim path waits on it)
  SOI FF D8 or fourcc MJPG → pixel_core::decode_jpeg (Annex K.3 tables before any DHT: Motion-JPEG decodes)
  YUY2 / NV12            → BT.601 limited range (UVC 1.5 §3.9.2.6 defaults), nearest-sampled into the surface
  first good frame → window "Camera" (aspect kept, never upscaled, ≤ 2/3 of the work area) → wm::present
  every 5 s: "[uvc] stream fps=<n> frames=<n> dropped=<n> bad=<n> src=<w>x<h> fourcc=<cc> shown=<w>x<h> render_us_max=<n> window=<open|closed-by-user|none-yet>"
  disarm / re-arm → the row closes, then the surface is freed; "[uvc] view end reason=… frames= dropped= bad= camera_frames="
```

`fps` is the CAMERA's assembled frames per second (the front buffer's sequence advance); `frames` the frames
presented; `dropped` the frames completed that a newer one replaced before the window showed them. A close by
the title-bar disc (`wm::close` clears the id through the holder registry) frees the surface on the next pass
and keeps counting; the window reopens with the next stream.

### 12.1 The fixture: `tests uvccam`

Over bytes written in the file from the tables, so it runs on QEMU with no camera:
the census of a 285-byte two-format camera (MJPEG 1920x1080 + 1280x720, YUY2 640x480 + 1280x720, one
isochronous alternate 1024 x 3) through CAMERA1's own `parse`, ~25 fields and the census line byte for byte;
a 26-byte PROBE/COMMIT block (Table 4-47) through `parse_probe`, resolved against the census (frame bytes =
w x h x bpp / 8 = `dwMaxVideoFrameSize`, payload ≤ the alternate's ESIT, a 25-byte runt refused); YUY2 and NV12
known colours (black, white, BT.601 red) and a 4→2 downscale; the tree's 8x8 JPEG with its four DHT segments
removed (a Motion-JPEG frame) decoded and sampled; the window box. Then, with a stream armed, 2 s of the
viewer's counters. Wire:

```
[uvc] dev=feed:0c0a formats=2 mjpeg=1920x1080@30 yuy2=640x480@30 frames=4
:: UVCCAM: parse=ok formats=2 probe=ok(static) convert=ok(yuy2,nv12,mjpeg) stream=skip(no-device) -> PASS ::     (QEMU)
:: UVCCAM: parse=ok formats=2 probe=ok(static) convert=ok(yuy2,nv12,mjpeg) stream=live fps=<n> frames=<n> dropped=<n> -> PASS ::   (bench)
```

A failed check prints `failures=<n> first=<field> got=<v> want=<v> -> FAIL`.

### 12.2 Owed by `drivers/uvc.rs` (the arc does not name that file; the seat folds these)

Until these four same-line folds land, the viewer never arms on the bench and the census line never prints
there; `tests uvccam` proves every pure half either way.

1. `stream_arm`, after `STREAM_ARMED.store(true, …)`: `crate::drivers::xhci::uvc::arm(addr, w, h, cc);`
2. `stream_disarm`, after `STREAM_ARMED.store(false, …)`: `crate::drivers::xhci::uvc::disarm();`
3. `probe_iso`, after `print_census(idx, addr, &call);`: `crate::drivers::xhci::uvc::census(addr, &call);`
   (`addr` is the xHCI slot on this path — `service_uvc` passes it — which is what `note_dev` keyed.)
4. `probe_iso`'s format pick commits only an UNCOMPRESSED format, so a camera offering MJPEG alone (or an
   uncompressed default frame past the 4 MiB store, e.g. 4K NV12) never streams. The change: when the
   uncompressed loop finds nothing, take the first `Kind::Mjpeg` format's default frame with `cc4 = *b"MJPG"`.
   The assembler needs no change (an MJPEG frame shorter than `dwMaxVideoFrameSize` is counted `short` and
   still published).

### 12.3 The bench (rMBP, the camera on a USB-A port)

1. The camera's USB connection mode set to its UVC mode (Sony calls it "USB Streaming"), cable to a USB-A port (xHCI; the internal FaceTime HD is EHCI and is not this path).
2. Boot the image line with `uvc` in it (kfeats carries it) and `UNAOS_WC=1`; at the desktop plug the camera in
   (or boot with it attached).
3. Read the wire top to bottom: `[uvc-iso] candidate slot=<n>` → `[uvc] dev=<vid:pid> formats=…` →
   `[uvc] commit=armed pipe=iso …` → `[uvc] view armed …` → `[uvc] view open win=<n> src=<w>x<h> …` → every
   5 s `[uvc] stream fps=<n> frames=<n> dropped=<n> …`. The picture is the window titled "Camera".
4. `tests uvccam` on the console: `stream=live fps=<n> frames=<n> dropped=<n> -> PASS`; `tests uvc` for the
   transport's own counters (missed / overrun / bad_hdr).
5. The first line that does not appear is the wall: §11.3's tree up to `commit=armed`; after it,
   `view armed` missing = §12.2's folds; `view open` missing with `stream fps>0` = `[uvc] view decode refused`
   or `unsupported fourcc` (the line names the FourCC); `fps=0` = §11.3 R6.
