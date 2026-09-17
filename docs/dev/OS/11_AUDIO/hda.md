# HDA — Intel High Definition Audio, the kernel's first audio line

`unaos/crates/kernel/src/drivers/hda.rs` · knobs `UNAOS_HDA=1` (arc 1) and `UNAOS_HDATONE=1`
(arc 2, implies arc 1) · Cargo features `hda` and `hda-tone = ["hda"]` · both default OFF ·
x86_64 only · rmbp-ledger **B127** · ROADMAP §6 row "Audio (kernel): x86 HDA".

**Nothing in this kernel had ever touched audio,** and the reason was structural rather than an
omission. `PciScanner::enumerate_buses` (`drivers/pci.rs:23`) matches exactly one class triple —
0x0C/0x03/0x30, xHCI — and returns on the first hit. `PciScanner::storage_inventory`
(`drivers/pci.rs:81`) matches class 0x01 and class 0x08/0x05. **Class 0x04 is a subclass space no
walk in this kernel could reach**, the same blind spot GR20 found for the class-0x02/0x80 radio, and
no capture in this project's history carries an `[hda]` line.

This document is the ladder: **arc 1** census → reset → CORB/RIRB → widget walk; **arc 2** the output
stream and its ear-free proof; **arc 3** owed.

---

## 0. Clean room

Every register offset, bit and verb in `drivers/hda.rs` is transcribed from the public **Intel High
Definition Audio Specification revision 1.0a** and the codec verb and parameter tables it defines.
No driver source from any other operating system was read while building it. Citation tags in the
source follow the tree's convention:

| tag | means |
| :--- | :--- |
| `[HDA-SPEC §x.y]` | the specification, section cited |
| `[TREE]` | a property of this codebase (the identity map, the TSC wait discipline, the DMA seam) |
| `[QEMU]` | a fact about the emulated fixture |
| `[METAL]` | a fact measured on the bench rMBP |

There is no `[METAL]` tag in the file yet. **Arc 1 is built and unflown**; §7 is what the first
metal boot has to show.

---

## 1. The ladder

| rung | knob | what it proves | state |
| :--- | :--- | :--- | :--- |
| 1a census | `UNAOS_HDA=1` | a class-0x04/0x03 function exists, and where | built |
| 1b claim + map | `UNAOS_HDA=1` | BAR0 decodes, bus master sticks, the register block reads | built |
| 1c reset | `UNAOS_HDA=1` | `GCTL.CRST` cycles and `STATESTS` names the codecs | built |
| 1d CORB/RIRB | `UNAOS_HDA=1` | the command ring round-trips — `GET_PARAMETER VENDOR_ID` answers | built |
| 1e walk | `UNAOS_HDA=1` | every widget, its connections, and the derived output path | built |
| 2 tone | `UNAOS_HDATONE=1` | a stream runs: LPIB advances and BCIS latches | **the second commit of this series** |
| 3a mixer seam | — | a volume/mute surface userspace can reach | **owed** |
| 3b Pi twin | — | BCM2711 HDMI / PWM audio, the other half of ROADMAP §6's row | **owed** |

Every rung above names the rungs below it as open (LAWS §5, the probe-ladder rule). A rung that
fails is "failed under `<conditions>`", never "ruled out"; its code and its knob stay.

---

## 2. Arc 1 — census, reset, rings, walk

### 2.1 The census (`drivers/pci.rs::audio_inventory`)

A separate pass, in the shape of the `[PCI-STOR]` storage census directly above it. It touches
neither `enumerate_buses` nor `storage_inventory`, issues **no config-space write** (BAR0 is read as
firmware left it — never sized, which would need the write-all-ones/restore dance — and no COMMAND
bit is touched), and prints one line per class-0x04 function:

```
[hda] census bdf=<b>:<s>.<f> id=<vvvv>:<dddd> class=04 sub=<ss> progif=<pp> (<kind>) bar0=<hex> irq=<n>
```

`kind` decodes subclass: `video` 0x00, `audio-legacy` 0x01 (AC'97), `telephony` 0x02, `hd-audio`
0x03, `multimedia-other` otherwise. Only subclass 0x03 is returned to the driver; the rest are
inventoried for the record. With no class-0x04 function anywhere, the driver prints `[hda] absent`.

`irq` is the Interrupt Line byte at config 0x3C. **This driver is polled end to end and never uses
it.** It is printed because it is the one fact an interrupt-driven arc has to start from, and
because a `255` is itself a finding (firmware routed the function nowhere).

### 2.2 Claim and map (`take`)

Memory decode **and** bus master are enabled, mask-disciplined — only bits 1 and 2 are set and
everything else is carried through — and the result is read back and refused out loud if it did not
stick. **Bus master is not optional here:** HDA has no PIO data path at all. The controller masters
the bus to fetch the CORB, write the RIRB, fetch the BDL and fetch sample data. A driver that mapped
BAR0 without it would program every register correctly and watch the RIRB write pointer never move.

BAR0 (config 0x10) is the register block \[HDA-SPEC §2.1\] and **may legally be a 64-bit BAR**,
unlike the AHCI ABAR case, where the register block is the last BAR of a type-0 header and therefore
cannot be. Both halves are decoded. An I/O BAR or an unassigned BAR0 is a named refusal, not a
silently wrong base.

The window is mapped with `arch::memory::map_mmio_window(base, 0x2000)` — the same seam
`drivers/ahci.rs`, `drivers/sdhc.rs` and the GPU drivers use, **uncacheable**, because the HDA
register block is a device aperture. 0x2000 covers the global registers (0x00..0x80) and all 32
stream descriptors (0x80 + 32 × 0x20) with room above; the identity map's leaves are 2 MiB, so this
types the containing leaf UC either way. Creating a mapping is a page-table edit, not a device
access: the controller sees nothing.

### 2.3 Reset (`reset`) \[HDA-SPEC §4.2.2, §3.3.7, §4.3\]

Both DMA engines are stopped **first** — a controller the firmware left running would keep fetching
across the reset window on some silicon. Then `GCTL.CRST` is driven low and **observed** low, held
100 µs, driven high and **observed** high. Then the specification's **521 µs codec-discovery
window** is waited out unconditionally (600 µs here) before `STATESTS` is read: reading it early is
how a driver decides a codec is absent that is merely slow.

`STATESTS` is RW1C. The value is read, then written back to clear the latches, so a later reader
does not see a state change that already happened.

```
[hda] gcap=<hex> oss=<n> iss=<n> bss=<n> nsdo=<n> 64ok=<0|1> version=<maj>.<min>
[hda] reset crst=1 statests=<hex> codecs=[<addrs>]
```

`GCAP` \[HDA-SPEC §3.3.2\]: bit 0 `64OK`, bits 2:1 `NSDO`, bits 7:3 `BSS`, bits 11:8 `ISS`,
bits 15:12 `OSS`. `OSS == 0` is a named refusal — there is nothing to program.

### 2.4 CORB and RIRB (`Rings::init`) \[HDA-SPEC §4.4.1.3, §4.4.2.2\]

256-entry rings (`CORBSIZE`/`RIRBSIZE` bits 1:0 = 2), 128-byte aligned, in heap memory the identity
map makes directly addressable by the controller — the same `bus_addr` seam every DMA structure in
`drivers/xhci/mod.rs` and `drivers/ahci.rs` uses, and the one function a non-identity-mapped arch has
to change.

The order is load-bearing and is the order in the code: stop both engines → check `CORBSZCAP`/
`RIRBSZCAP` for the 256-entry bit → write sizes and bases → reset `CORBRP` (set `CORBRPRST`, wait
for the readback, clear it, wait for zero) → zero `CORBWP` → reset `RIRBWP` → `RINTCNT` = 1 → **only
then** set `CORBRUN` and `RIRBDMAEN`. Every failure along the way is a `[hda] rings REFUSED
reason=<token>` line with the register that refused.

⚠ **`RIRBCTL.RINTCTL` IS SET, IN A DRIVER THAT TAKES NO INTERRUPTS, AND THAT IS NOT A
CONTRADICTION.** It gates the `RIRBSTS.RINTFL` **latch**, not only the interrupt. With it clear, the
controller counts responses against `RINTCNT`, reaches the count, **stops fetching commands**, and
never sets the status bit whose RW1C would release it. This cost two QEMU runs to find, and the
instrument that found it is now permanent — a one-shot `[hda] ring-state` line on the boot's first
response:

```
[hda] ring-state after-first-verb corbwp=0x0001 corbrp=0x0001 rirbwp=0x0001 rirbsts=0x00->0x00 ...
[hda] verb TIMEOUT word=0x000f0002 corbwp=0x0002 corbrp=0x0001 rirbwp=0x0001 rirbsts=0x00 ...
```

One response had landed and the latch was still zero; the next command then sat unfetched forever,
and the driver read that as a codec that had stopped answering. With `RINTCTL` set the same line
reads `rirbsts=0x01->0x00` — latched, then cleared by `issue` — and the walk completes.

**No message reaches the CPU, and that claim is separate from this bit.** Interrupt *generation* is
gated by `INTCTL.GIE`/`CIE` \[HDA-SPEC §3.3.14\], which this driver never writes. The `[hda] rings`
line prints `intctl=<hex>(untouched)` and the audit line carries `wrote-intctl=0(audited)`, so a
reader checks it from the wire rather than from this paragraph.

A verb round trip is strictly serialised: one command outstanding at a time, a `SeqCst` fence between
the CORB entry store and the `CORBWP` advance (a store-ordering fence, not a cache flush — the heap
is write-back and DMA is snooped on this arch), and a 100 ms deadline on the RIRB write pointer. A
codec answers in microseconds; 100 ms is short enough that a whole widget walk against a dead codec
still ends inside a boot, and a timeout prints `[hda] verb TIMEOUT` with `CORBWP`, `CORBRP`,
`RIRBWP` and `RIRBSTS`.

### 2.5 The widget walk (`walk_codec`) \[HDA-SPEC §7.1.2, §7.3.3, §7.3.4\]

Per codec: `GET_PARAMETER VENDOR_ID` (0x00) and `REVISION_ID` (0x02) at node 0, then
`SUBORDINATE_NODE_COUNT` (0x04) for the function-group range; per function group,
`FUNCTION_GROUP_TYPE` (0x05), and for an **audio** group (type 0x01) a `SET_POWER_STATE D0` before
anything is read — a codec in D3 is entitled to answer a parameter read with stale or zeroed state —
then its own subordinate-node range and one pass over every widget.

Per widget: `AUDIO_WIDGET_CAPS` (0x09) for the type (bits 23:20) and the amp/connection/power/digital
bits; the **connection list** through `CONNECTION_LIST_LENGTH` (0x0E) and `GET_CONNECTION_ENTRY`
(0xF02), decoded in both the short form (four 8-bit entries per response) and the long form (two
16-bit entries), with the **range** encoding handled — an entry whose top bit is set is the inclusive
end of a range starting at the previous entry; and for a pin complex, `PIN_CAPS` (0x0C) and
`GET_CONFIG_DEFAULT` (0xF1C).

One line per node, with the pin default configuration fully decoded \[HDA-SPEC §7.3.3.31\]:

```
[hda] node=0x<nn> type=pin conns=[..] pincap=<hex> out=<0|1> hp=<0|1> eapd=<0|1> \
      pincfg=<hex> (dev=<device> loc=<location> conn=<connection-type> colour=<colour> \
      portconn=<n> assoc=<n> seq=<n>)
[hda] node=0x<nn> type=<kind> conns=[..] caps=<hex> out-amp=<0|1> in-amp=<0|1> \
      amp-override=<0|1> power-ctl=<0|1> digital=<0|1> pincfg=-
```

`amp-override=0` is printed and matters: with the override bit clear a widget's amplifier
capabilities are the function group's defaults rather than its own, so a reader comparing two nodes'
gain ranges needs to know which of the two they are looking at.

Widget types are the spec's: `audio-out` 0, `audio-in` 1, `mixer` 2, `selector` 3, `pin` 4, `power`
5, `volume-knob` 6, `beep` 7, `vendor-defined` 0xF.

### 2.6 Path derivation

After the whole graph is known — a pin's converter can have a higher NID than the pin, so this
cannot be done inside the walk loop — every **output-capable** pin (`PIN_CAPS` bit 4) whose port
connectivity is not "no physical connection" is searched backwards through connection lists to an
Audio Output widget. The search is a depth-first walk bounded by `MAX_PATH_DEPTH` (8) **and** by a
visited set, so a cyclic or self-referential connection list ends the search rather than the boot.
A pin that is output-capable and reaches no converter prints its own line and is not a candidate.

Candidates are **ranked**, and the rank is in the source rather than in prose:

| rank | pin |
| :--- | :--- |
| 0 | `dev=speaker` **and** `loc=internal` — the rMBP's target |
| 1 | `dev=speaker`, any location |
| 2 | `dev=hp-out` |
| 3 | `dev=line-out` |
| +8 | any of the above on a **digital** widget |

⚠ **The brief said "speaker path"; the code ranks output devices.** That widening is deliberate and
is named here rather than buried: QEMU's `hda-duplex` codec exposes a **line-out** and no speaker at
all, so a speaker-only match would make the whole QEMU fixture refuse and arc 2 would have no gate
anywhere. The chosen device is **printed** on the path line, so a reader always knows which of the
three a given boot drove, and `speaker_pin=` on the summary still reports only a real `dev=speaker`
pin (or `none` — never `0x00`, which is a legal NID).

```
[hda] path codec=<n> dac=0x<nn> -> [<nodes>] -> pin=0x<nn> dev=<device> hops=<n>
[hda] walk codecs=<n> nodes=<n> dacs=<n> pins=<n> speaker_pin=<0xnn|none> hp_pin=<0xnn|none> path=<found|none>
:: HDA: codecs=<n> nodes=<n> dacs=<n> pins=<n> path=<found|none> -> PASS|FAIL ::
```

The verdict line is printed **only** on a machine whose controller reset, answered and was walked, so
its silence means "no HDA controller answered", never "the walk passed" (LAWS §5: an absence is
evidence only if the producing path ran). A `-> FAIL` reds any leg through `arroyo`'s FAULT-SCAN list
and `mbench`'s `DEFAULT_FORBIDS`.

---

## 3. Arc 2 — the tone

> ⚠ **Arc 2 lands in the SECOND commit of this series.** This section describes what that commit
> builds; at this commit `drivers/hda.rs` ends at its arc-2 banner and writes no stream register.

`UNAOS_HDATONE=1`. **This is the one knob in the tree that makes an audible noise in the room**,
which is why it is its own feature and not part of `hda`: a boot that did not ask for a tone must be
structurally incapable of emitting one. That is the construction `btc` uses for the Bluetooth page,
and for the same reason.

### 3.1 The signal

16-bit stereo, 48 kHz, 440 Hz, one second, amplitude 4096 of 32767 (about −18 dBFS). The sine is
computed in fixed point — the kernel has no float runtime — as an odd polynomial on the first
quadrant mirrored into the other three, accurate to better than the quantisation of a 16-bit sample.

48000 frames × 4 bytes = **192000 bytes, which is 1500 × 128**, so the cyclic buffer length is a
multiple of 128 bytes as the specification requires \[HDA-SPEC §3.3.38\]. The BDL has **two entries**
— the minimum the specification allows \[HDA-SPEC §3.6.2\] — each half the buffer, each with IOC set,
so `BCIS` latches twice per pass.

### 3.2 The descriptor

**Output stream 0's descriptor is at `0x80 + ISS × 0x20`, not at `0x80`.** The input stream
descriptors come first in the array \[HDA-SPEC §3.3.35\]; a driver that assumed descriptor 0 was an
output stream would program an *input* engine and then report a stuck LPIB as a broken codec. `ISS`
comes from `GCAP` and is printed on the arm line.

Programming order: stream reset (`SRST` set, observed, cleared, observed) → `BDPL`/`BDPU` → `CBL` →
`LVI` → `FMT` → clear the sticky `BCIS`/`FIFOE`/`DESE` bits → stream tag into `SDnCTL[23:20]` →
`IOCE` → **then** `RUN`. `FMT` and `CBL` are read back and both values are printed beside what was
written.

Format `0x0011` \[HDA-SPEC §3.3.41 table 53\]: BASE 0 (48 kHz family), MULT 000, DIV 000, BITS 001
(16-bit), CHAN 0001 (two channels, encoded as count − 1). The same word goes to the converter through
`SET_CONVERTER_FORMAT`, **before** `SET_CONVERTER_STREAM_CHANNEL` — a converter bound to a stream
before its format is set can latch the old format for the first buffer.

Stream tag is **1**. Tag 0 means "unused": a stream programmed with tag 0 is one the link will never
carry, which is exactly the go-red mutation §6 names.

### 3.3 The codec side of the path

For every node on the derived path, in path order: `SET_POWER_STATE D0` where the widget declares
power control; `SET_CONNECTION_SELECT` on a **selector** with more than one input, pointed at the
index the path actually took (a **mixer** is left alone — it sums — and gets its *input* amplifier
unmuted instead); and `SET_AMPLIFIER_GAIN_MUTE` unmuted at a **moderate** gain.

"Moderate" is the codec's own 0 dB offset — bits 6:0 of the amp-capabilities word \[HDA-SPEC
§7.3.4.10, §7.3.4.12\] — clamped to the declared step count, never the maximum. This is a diagnostic
tone on a laptop speaker; a full-scale sine out of a cold boot is how you frighten an operator.

The pin gets `SET_PIN_WIDGET_CONTROL` with OUT_ENABLE, plus the headphone amplifier bit when the pin
declares headphone drive, and `SET_EAPD_BTL_ENABLE` with the EAPD bit where `PIN_CAPS` bit 16 says
the pin has one — **that is the external amplifier the rMBP's internal speakers hang off**, and a
tone programmed without it is a tone nobody hears.

### 3.4 The proof, without ears

```
[hda] tone stream=0 lpib=<start> -> <end> (max <n>) bcis=<n> fifo_ready=<0|1> run_ms=<n> \
      sts=<hex> fifoe=<0|1> dese=<0|1> cbl=<n> tag=<n>
:: HDA-TONE: lpib_advanced=<0|1> bcis=<n> fifo_ready=<0|1> run_ms=<n> -> PASS|FAIL ::
```

The claim is "the stream ran", and it has **two independent witnesses**: the link position advanced
(the controller fetched and consumed sample data) and a buffer boundary latched `BCIS` (the BDL was
walked to the end of at least one descriptor). Either alone is weaker than both — LPIB can be read
mid-fetch on a stream that stalls immediately after, and a stale `BCIS` is impossible because the bit
is cleared above before `RUN`. `FIFOE` or `DESE` set at the end fails the verdict outright.

`BCIS` latches because `IOCE` is set in `SDnCTL`; it reaches no CPU because `INTCTL` stays 0. It is a
polled flag, and the file says so at the top.

### 3.5 Refusal and restore

On a codec whose walk derived no output path, arc 2 prints

```
[hda] tone REFUSED reason=no-speaker-path
:: HDA-TONE: reason=no-speaker-path -> REFUSED ::
```

and issues **not one stream register write**. `REFUSED` is not `FAIL`: a machine with no output path
is a machine this arc has nothing to say about, and reddening a leg for it would train the eye to
skip the token.

Everything the run changed is read **before** it is written and written **back** after: `SDnCTL` (all
three bytes), `SDnFMT`, `SDnBDPL`, `SDnBDPU`, `SDnCBL`, `SDnLVI`, the sticky status bits, and on the
codec side the pin control, the EAPD byte, the converter format, the stream/channel binding, every
power state and every output amplifier gain-and-mute on the path. The stream is stopped and put
through a full `SRST` cycle before any of it.

---

## 4. The write audit

Every write is counted and printed in the shape `drivers/bcma.rs` established — a number a reader can
check against the code, and an `(audited)` zero for a class this driver deliberately never issues:

```
[hda] audit stage=<walk|tone|end> wrote-cfg=<n> wrote-ctrl=<n> wrote-stream=<n> \
      verbs-get=<n> verbs-set=<n> wrote-intctl=0(audited) wrote-wallclk=0(audited) \
      wrote-dplbase=0(audited)
```

`wrote-cfg` is at most 1 and is the COMMAND register only. `wrote-intctl=0` is the interrupt claim
made checkable from the wire. `wrote-dplbase=0` says the DMA position buffer is not used — LPIB is
the position source, and a position buffer would be a second thing to keep coherent for no gain at
enumeration time.

---

## 5. Where the driver is hooked, and where it should be

The probe is called from a **line-neutral append on `drivers/pci.rs:24`**, the first statement of
`PciScanner::enumerate_buses`, under `#[cfg(all(target_arch = "x86_64", feature = "hda"))]`.

That is the only unconditionally-executed statement in a file this arc's brief names: `scan()`'s
other two statements are the arms of an `if let` on the xHCI result, so a hook on either would run on
one machine and not the other. Everything the driver needs is up by then — `pci::init` runs at
`main.rs:1034`, the heap at `main.rs:296`, and `bcma::recon` maps an MMIO BAR six lines above this
function's own call site (`arch/x86_64/pci.rs:875` against `:881`).

⚠ **It charges HDA bring-up to the `pci-scan` BPACE delta** stamped at `arch/x86_64/pci.rs:882`. That
is a default-OFF dev knob so no shipped boot pays it, and the cost is named here rather than hidden.
**The placement a future fold should prefer is the AHCI-shaped line-neutral append at
`arch/x86_64/pci.rs:1048`**, which sits outside every pacing accumulator and next to the other
enumeration hooks. `arch/x86_64/pci.rs` was not in this arc's brief and was therefore not touched;
moving the statement is a one-line change in each of the two files and needs no change to this
driver.

---

## 6. Gates and go-red

### The QEMU fixture

This is the rare driver in this neighbourhood with a **real emulator** — unlike the BCM4331 radio or
the Apple SMC, whose arcs are metal-first by construction. `builder/src/main.rs` attaches, under the
same `UNAOS_HDA` / `UNAOS_HDATONE` env vars that push the kernel features:

```
-device intel-hda,id=hda0 -device hda-duplex,bus=hda0.0,audiodev=snd0 -audiodev none,id=snd0
```

`audiodev=` is a required property of the codec device on the QEMU this tree builds against, and
`none` is a real backend that consumes samples and produces silence — the stream engine runs, LPIB
advances and BCIS latches with nothing reaching the host's sound card. Unset, **not one argument is
added** and a default run's QEMU command line is byte-identical to what it was before this arc.

The knob is a builder knob **and** a kernel feature read from the same variable, which is the one
shape the tree's four-place wiring note does not otherwise cover: the fixture and the driver can
never disagree about whether a given run has a controller in it.

### The runs

```
UNAOS_HDA=1 ./arroyo test 60
UNAOS_HDA=1 UNAOS_HDATONE=1 ./arroyo test 60
```

### Go-red by mutation

The arc's falsifier is **the stream tag**, and it is the mutation the DONE gate ran: set
`STREAM_TAG` to 0 in `drivers/hda.rs` and rebuild. A stream tagged 0 is one the link never carries,
so the fixture's `[hda] tone` line reads a **stuck LPIB and `bcis=0`**, and `:: HDA-TONE: … -> FAIL ::`
reds the leg through `arroyo`'s FAULT-SCAN list. Reverting restores the pass. The other available
mutation is the **format** — any `SDnFMT` the codec does not support — which fails the same way
through a different mechanism.

### Byte identity

Both knobs are `./arroyo knoboff hda` / `./arroyo knoboff hda-tone`, exit 0 (byte-identical), with
`warm=yes` quoted beside the verdict. The shape that earns it:

- `drivers/hda.rs` is **not lexed at all** knob-off — the `#[cfg]`-erased `pub mod` in
  `drivers/mod.rs` is the one case LAWS §5 names as byte-safe for a module — and the declaration is
  the **last** line of that file, so no module above it moves.
- `drivers/pci.rs::audio_inventory` is an **impl-tail** append and the probe call is a **line-neutral**
  append on an existing line, before that line's first `//` (LEDGER P7).
- Every arc-2-only constant, the `cmd16` verb form, `elapsed_ms` and the whole `mod tone` block carry
  `#[cfg(feature = "hda-tone")]`, so `hda` alone lexes none of them.
- `arroyo`'s `arm_features` strips both names, so neither knob can shift an aarch64 `-Cmetadata`
  fingerprint. ⚠ `drivers/pci.rs` **is** lexed on aarch64, unlike `drivers/ahci.rs` — which is why
  both of its sites carry `target_arch = "x86_64"` **in the attribute** rather than relying on the
  module gate; the `gen7` entry in `arm_features` records the review that caught exactly that shape.

---

## 7. The metal expectation

The bench machine is a 2012 15" Retina MacBook Pro, MacBookPro10,1, Intel 7-series (Panther Point)
PCH. **Expected, not assumed** — the census prints what is there:

| fact | expectation | how it is settled |
| :--- | :--- | :--- |
| controller bdf | `0:27.0` | the `[hda] census` line |
| controller id | `8086:1e20` (7-series HD Audio) | the `[hda] census` line |
| class triple | `class=04 sub=03 progif=00` | the `[hda] census` line |
| codec | Cirrus Logic **CS4206** — vendor 0x1013 | **to be read**, from `[hda] codec=0 vid=…` |
| codec count | 1 | `[hda] reset … codecs=[…]` |
| internal speaker pin | `dev=speaker loc=internal`, EAPD-capable | `[hda] node=…` and `speaker_pin=` |
| headphone pin | `dev=hp-out loc=external` | `hp_pin=` |
| the tone | `lpib` advancing, `bcis` ≥ 2 at one second, and **Peter hears 440 Hz from the internal speakers** | `[hda] tone` plus the room |

The codec's vendor/device id is the one line of §7 nobody in this tree has ever read. A discrete
GPU's HDMI audio function may also appear in the census on this machine — the GK107 has one — which
is why the census prints every class-0x04 function and the driver claims the first that resets and
answers. Driving two controllers at once is an arc 3 question and is deliberately not done quietly
here.

**Flight line:**

```
UNAOS_HDA=1 UNAOS_HDATONE=1 ./arroyo esp-x86
```

---

## 8. Arc 3 — owed

1. **A mixer and volume seam for userspace.** The walk already derives the path and reads every
   amplifier's capability word, so the missing piece is a surface, not a discovery: a gain/mute call
   bound to the derived path's nodes, reachable from a handler, with the same capability discipline
   `drivers/block.rs`'s `WriteGrant` uses for a disk write. `handlers/stria` is the host-native
   consumer that exists already (ROADMAP §3a) and the kernel side is what this row owes it.
2. **The Pi twin.** ROADMAP §6's audio row is *two* gaps — x86 HDA and BCM2711 HDMI/PWM — and this
   arc closes one. The Pi's path is a different controller entirely (no CORB/RIRB, no codec verbs)
   but the same shape above it: derive a path, program a stream, prove it ran without ears. The
   seam that should be shared is the *proof*, not the register map.
3. **Interrupts.** `INTCTL`, `RIRBCTL.RINTCTL` and the per-stream interrupt enables are all
   deliberately untouched and audited as zero. Any arc that wants continuous playback rather than a
   one-second diagnostic needs them, and needs a service-pass owner for the stream that this arc
   (one pass inside `pci::init`) does not have.
4. **Multi-codec and multi-controller.** `STATESTS` is walked in full and every present codec is
   walked, but only the first with a derived path is a tone candidate, and only the first controller
   that answers is claimed.
