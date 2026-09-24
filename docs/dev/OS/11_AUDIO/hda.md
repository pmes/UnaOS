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
stream and its ear-free proof; **arc 3** owed; **arc 4** the second controller.

**FLOWN 2026-09-22 (flight 11).** Arc 1 passed on its first metal boot and read
`[hda] codec=0 vid=1013:4206` — a Cirrus Logic CS4206. Arc 2 ran a full second of samples at the
exact link rate and **the room was silent**. §6 has the wire, the arithmetic and the three defects
that reading named; rmbp-ledger **B127** (flown) and **B130**.

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

**Arc 1 and arc 2 have now flown** (flight 11, 2026-09-22): §6 carries the measured wire and §7's
expectation table is settled against it. A `[METAL]` tag in the source marks a fact that boot
supplied.

---

## 1. The ladder

| rung | knob | what it proves | state |
| :--- | :--- | :--- | :--- |
| 1a census | `UNAOS_HDA=1` | a class-0x04/0x03 function exists, and where | built |
| 1b claim + map | `UNAOS_HDA=1` | BAR0 decodes, bus master sticks, the register block reads | built |
| 1c reset | `UNAOS_HDA=1` | `GCTL.CRST` cycles and `STATESTS` names the codecs | built |
| 1d CORB/RIRB | `UNAOS_HDA=1` | the command ring round-trips — `GET_PARAMETER VENDOR_ID` answers | built |
| 1e walk | `UNAOS_HDA=1` | every widget, its connections, and the derived output path | built |
| 2 tone | `UNAOS_HDATONE=1` | a stream runs: LPIB advances and the BDL is walked to its end (a WRAP or a BCIS latch) | **flown, silent** |
| 2b amp | `UNAOS_HDATONE=1` | the codec's whole declared GPIO set driven, because this codec has no EAPD | built, **unflown** |
| 2c pair | `UNAOS_HDATONE=1` | every pin of the speaker association driven, sequence-ordered, one stream tag | built, **unflown** |
| 3a mixer seam | — | a volume/mute surface userspace can reach | **owed** |
| 3b Pi twin | — | BCM2711 HDMI / PWM audio, the other half of ROADMAP §6's row | **owed** |
| 4 second controller | — | the GK107's HDMI audio at `1:0.1`, enumerated on flight 11 and not claimed (§9) | **owed** |

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

⚠ **"The path" is now every member of the derived pin's ASSOCIATION, not one pin.** An association
is a group of pins wired as one device and its *sequence* field orders them, sequence 0 being the
primary \[HDA-SPEC §7.3.3.31\]; the rMBP's internal speakers are two pins of association 1 with
**different** converters, and flight 11 drove only one of them (§6, defect 3). Arc 2 collects every
output-capable pin of that association carrying the same default device and owning a converter of
its own (bounded at `PAIR_MAX = 2`), orders them by sequence, and applies everything below to each —
binding all members to the **same stream tag**, with the starting channel \[HDA-SPEC §7.3.3.11\] 0
for a stereo converter and the sequence index for a mono one. Every member prints its own
`[hda] pair`, `[hda] bind`, `[hda] power` and `[hda] amp` lines, and one member that did not take
its binding fails the whole run.

For every node on each member's path, in path order: `SET_POWER_STATE D0` where the widget declares
power control; `SET_CONNECTION_SELECT` on a **selector** with more than one input, pointed at the
index the path actually took (a **mixer** is left alone — it sums — and gets its *input* amplifier
unmuted instead); and `SET_AMPLIFIER_GAIN_MUTE` unmuted at a **moderate** gain.

"Moderate" is the codec's own 0 dB offset — bits 6:0 of the amp-capabilities word \[HDA-SPEC
§7.3.4.10, §7.3.4.12\] — clamped to the declared step count, never the maximum. This is a diagnostic
tone on a laptop speaker; a full-scale sine out of a cold boot is how you frighten an operator.

The pin gets `SET_PIN_WIDGET_CONTROL` with OUT_ENABLE, plus the headphone amplifier bit when the pin
declares headphone drive, and `SET_EAPD_BTL_ENABLE` with the EAPD bit where `PIN_CAPS` bit 16 says
the pin has one.

⚠ **EAPD IS NOT THE rMBP'S SPEAKER AMPLIFIER, AND THIS PARAGRAPH USED TO SAY IT WAS.** The claim
"that is the external amplifier the rMBP's internal speakers hang off" was written before any metal
boot and flight 11 falsified it: **both** internal speaker pins report `eapd=0` (`PIN_CAPS` bit 16
clear), so the verb is correctly skipped and nothing wakes the amplifier. The codec-side output the
specification *does* still offer is the function group's **GPIO** set, which arc 2 now drives as a
whole and restores at stop — see §6, defect 2, for what the specification does and does not license
here and why the ear is the instrument that narrows it.

### 3.4 The proof, without ears

```
[hda] tone stream=0 lpib=<start> -> <end> (max <n>) bcis=<n> fifo_ready=<0|1> run_ms=<n> \
      sts=<hex> fifoe=<0|1> dese=<0|1> cbl=<n> tag=<n> tag_bound=<hex> tag_ok=<0|1> \
      wraps=<n> consumed=<bytes> rate_bps=<n> expect_bps=192000 members=<n> ctl_running=<hex>
:: HDA-TONE: lpib_advanced=<0|1> walked=<0|1> wraps=<n> bcis=<n> tag_ok=<0|1> fifo_ready=<0|1> \
      run_ms=<n> members=<n> -> PASS|FAIL ::
```

The claim is "the stream ran", and it has **three witnesses, of two different kinds**.

**Controller side.** The link position advanced (the engine fetched and consumed sample data) **and
the BDL was walked to its end** — proven by `wraps > 0` (a full pass of the whole cyclic buffer, so
of *every* descriptor in the list) **or** by a `BCIS` latch (one descriptor boundary). Either
witness alone is weaker than the pair: LPIB can be read mid-fetch on a stream that stalls
immediately after. `FIFOE` or `DESE` set at the end fails the verdict outright.

⚠ **`BCIS` used to be that second witness on its own, and metal took it away.** Flight 11 read
`bcis=0` on the Intel 7-series PCH with IOC set in both BDL entries and `SDnCTL.IOCE` set, on a run
whose LPIB walked the entire 192000-byte buffer and wrapped — so a working stream was scored `FAIL`
by a status bit this controller does not latch. The term is now `wraps > 0 || bcis > 0`, which is
**stronger**, not laxer; `bcis` stays on the wire, unscored alone, and *why* this silicon does not
latch it is §6's open rung. Neither bit reaches a CPU either way: `INTCTL` stays 0.

⚠ **LPIB IS A CYCLIC POSITION, NOT A TOTAL**, and reporting only its final value is how flight 11's
full-rate run read as a stall. The loop counts wraps and the line carries `consumed=` and
`rate_bps=` beside `expect_bps=192000`; the rate is printed and deliberately **not** a verdict term,
because QEMU's `audiodev none` backend has no reason to consume at wall-clock rate and gating on a
tolerance band there is how a gate becomes a flake.

**Link side — and it exists because the controller-side pair was measured to be blind to it.** The
first version of this arc scored only LPIB and BCIS, and the go-red the brief named (stream tag 0,
the "unused" tag no link ever carries) **did not fail it**:

```
[hda] tone stream=0 lpib=0 -> 44160 (max 191960) bcis=2 fifo_ready=1 run_ms=1200 ... tag=0
:: HDA-TONE: lpib_advanced=1 bcis=2 fifo_ready=1 run_ms=1200 -> PASS ::
```

That is an honest reading of what those two witnesses see: **the stream engine ran**. They live on
the controller and say nothing about whether the link carries the samples to a converter. So the
binding is now checked where it *is* observable — the converter is asked, with
`GET_CONVERTER_STREAM_CHANNEL`, what it was bound to, and `tag_ok` folds that readback plus
`tag != 0` into the verdict. It is a programming invariant read at the wire, not a restatement of
the write.

**What none of the three can see is the room.** A `PASS` with silence from the speakers is a real
finding on metal and names the next rung; §7 says so.

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

Flight 11 measured `stage=tone wrote-cfg=1 wrote-ctrl=184 wrote-stream=22 verbs-get=69 verbs-set=13`
on the CS4206. **`verbs-set` and `verbs-get` both rise** with the association pair, the GPIO set and
the readback lines — a second member roughly doubles the codec-side programming, the GPIO set costs
three `SET`s and six `GET`s plus three `SET`s at restore, and the `[hda] power` / `[hda] amp` /
`[hda] bind` lines are `GET`s only. `wrote-cfg`, `wrote-intctl`, `wrote-wallclk` and `wrote-dplbase`
are all unchanged, and that is the point of counting them separately: **the classes of write this
driver refuses to make did not move.**

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

### Flight 11 — the first metal boot, and the three defects it named (2026-09-22)

The arc flew on the bench rMBP on 2026-09-22. Capture slice:
`~/unaos-bench/scratch/rmbp-0915/hdaamp-logs/f11.log`, read with
`awk 'index($0,"[hda]")'`. **Arc 1 passed on its first metal boot**, and the line nobody in this
tree had ever read came back:

```
[hda] census bdf=0:27.0 id=8086:1e20 class=04 sub=03 progif=00 (hd-audio) bar0=0xc1c10000 irq=0
[hda] census bdf=1:0.1 id=10de:0e1b class=04 sub=03 progif=00 (hd-audio) bar0=0xc1080000 irq=0
[hda] gcap=0x4401 oss=4 iss=4 bss=0 nsdo=0 64ok=1 version=1.0
[hda] reset crst=1 statests=0x0001 codecs=[0]
[hda] codec=0 vid=1013:4206 rev=0x00100302
[hda] walk codecs=1 nodes=20 dacs=5 pins=10 speaker_pin=0x0a hp_pin=0x09 path=found
```

Cirrus Logic **CS4206**, one codec, twenty widgets, five converters, ten pins — §7's expectation
settled from the wire, not assumed. Arc 2 armed cleanly and **Peter heard nothing**:

```
[hda] node=0x0a type=pin conns=[3] pincap=0x00000054 out=1 hp=0 eapd=0 pincfg=0x90100112 (dev=speaker loc=internal … assoc=1 seq=2)
[hda] node=0x0b type=pin conns=[4] pincap=0x00000050 out=1 hp=0 eapd=0 pincfg=0x90100110 (dev=speaker loc=internal … assoc=1 seq=0)
[hda] path codec=0 dac=0x03 -> [10, 3] -> pin=0x0a dev=speaker hops=2
[hda] tone arm sd=0 (iss=4 => descriptor 4) fmt=0x0011(readback 0x0011) cbl=192000(readback 192000) lvi=1 … tag=1 srst=1/1 ctl=0x00140004
[hda] tone stream=0 lpib=0 -> 38396 (max 192000) bcis=0 fifo_ready=1 run_ms=1200 sts=0x20 fifoe=0 dese=0 cbl=192000 tag=1 tag_bound=0x10 tag_ok=1
```

#### ⚠ Defect 1 — THE DMA DID NOT STALL. THE VERDICT LINE CANNOT COUNT.

The obvious reading of `lpib=0 -> 38396 (max 192000)` is "the engine stopped a fifth of the way
through the buffer". **The same line falsifies it.** `SDnLPIB` is a position *inside* the cyclic
buffer and returns to zero every `SDnCBL` bytes \[HDA-SPEC §3.3.37\], and this run reports
`max 192000` — the engine reached the end of the whole 192000-byte buffer — with a *final* position
of 38396, which is only possible after a wrap. So the bytes consumed are

| term | value | source |
| :--- | ---: | :--- |
| one full pass of the cyclic buffer | 192000 | `max 192000` = `cbl=192000` |
| position in the second pass | 38396 | `-> 38396` |
| **consumed in 1200 ms** | **230396** | sum |
| **measured rate** | **191997 B/s** | 230396 × 1000 / 1200 |
| required rate, 48 kHz 16-bit stereo | 192000 B/s | `fmt=0x0011` \[HDA-SPEC §3.3.41\] |

**−0.0017 %.** The stream engine ran at the exact link rate for the entire 1.2 s. Every
controller-side suspect the flight went looking for is therefore excluded by the flight's own wire:
the converter format matches `SDnFMT` (`fmt=0x0011(readback 0x0011)`), the cyclic buffer length took
(`cbl=192000(readback 192000)`), the tag bound (`tag_bound=0x10 tag_ok=1`), `fifo_ready=1`,
`fifoe=0`, `dese=0`. **The silence is entirely codec-side**, and that is what turns the flight from
"why did the DMA stop" into defect 2.

What *is* defective is the instrument. Two things:

1. **The line reported a cyclic position as if it were a total.** Fixed: the run loop counts wraps
   (`l < prev`) and the line now carries `wraps=`, `consumed=`, `rate_bps=` and `expect_bps=`. A
   future reader does not have to redo the arithmetic above.
2. **`bcis=0` with IOC set in both BDL entries and `SDnCTL.IOCE` set** (`ctl=0x00140004`, bit 2).
   The completion latch did not fire on this controller although the engine walked past the end of
   *both* descriptors. That made `ok` false on a stream that demonstrably ran — the verdict voted
   down a working stream because a status bit did not latch. Fixed by replacing the second witness
   with a **strictly stronger** one: `walked = wraps > 0 || bcis > 0`. A wrap is a full pass of
   every descriptor in the list; a BCIS latch is one descriptor boundary. `bcis` stays on the wire,
   unscored alone.
   **The open question, for the next flight:** *why* this silicon does not latch BCIS. The leading
   hypothesis has a precedent in this very file — `RIRBCTL.RINTCTL` gates the `RIRBSTS.RINTFL`
   LATCH and not only the interrupt (§2.4, measured over two QEMU runs) — so the symmetric
   candidate is that on the Intel 7-series PCH `INTCTL.SIE[n]` \[HDA-SPEC §3.3.14\] gates the
   `SDnSTS.BCIS` latch the same way. It was **not** tried this flight: `INTCTL` is this driver's
   audited never-written register (`wrote-intctl=0(audited)`), setting `SIE` with `GIE` clear
   generates no message and is safe, and it is a one-knob experiment — but it is a change to a
   stated invariant and belongs in a flight of its own, now that the verdict no longer needs it.

   Two smaller observations from the same line, recorded so they are not re-derived: `sts=0x20` is
   `FIFORDY` alone, and `ctl=0x00140004` carries **bit 18 (`TP`, traffic priority) set although the
   driver never wrote it** — the byte-2 write is `0x10` and the readback is `0x14`, so that bit is
   the controller's, not ours. The new arm line prints `lvi=…(readback …)` and `ioce=` beside it,
   and every BDL entry is now read back and printed (`[hda] bdl entry=… addr=… len=… ioc=…`), which
   is where a wrong length or a lost IOC flag would show.

#### ⚠ Defect 2 — THE SPEAKER AMPLIFIER IS NOT AN EAPD PIN ON THIS CODEC

Both internal speaker pins report **`eapd=0`** — `PIN_CAPS` bit 16 clear, `pincap=0x00000054` on
0x0a and `0x00000050` on 0x0b \[HDA-SPEC §7.3.4.9\]. The external-amplifier bit arc 2 was written
to drive **does not exist on this part**, so `VERB_SET_EAPD` was correctly skipped and *nothing at
all* was done to wake the speaker amplifier. §7's expectation row said "EAPD-capable"; the wire says
no, and the row is now settled the other way.

The specification defines exactly one other codec-side output: the function group's **GPIO pins**
\[HDA-SPEC §7.3.4.14 for `PARAM_GPIO_COUNT` (0x11); verbs 0xF15/0x715 data, 0xF16/0x716 enable,
0xF17/0x717 direction\]. It does **not** define what any of them is wired to — that is the
machine's wiring, not the standard's, and this tree has no legal source for it (clean room, §0). So
the arc does the only spec-legal thing available: it reads the declared GPIO/GPO/GPI counts, drives
**the whole declared set as one** (enable, then direction, then data — a pin driven before it is an
enabled output is a write to a pin the codec is not driving), prints every word it read and wrote,
and restores all three registers in mirror order at stream stop. Driving them one at a time would
need a boot per GPIO and the specification gives no reason to prefer any order; **the ear answers
whether the set matters, and the next flight narrows within it.** A codec that declares no GPIOs —
QEMU's `hda-duplex` — prints `gpio … -> none` and is not written at all.

Beside it, the four codec registers a silent-but-running stream must be diagnosed from, none of
which flight 11 printed: `[hda] power` (the `F05` readback's *set* nibble **and** its *actual*
nibble, on the pin and the converter, against the entry value), and `[hda] amp` (the `0xB00`
gain/mute readback on converter and pin, the pin-control readback with `out_en`/`hp_en` decoded, and
the converter's **own** format against `SDnFMT`).

#### ⚠ Defect 3 — AN ASSOCIATION IS A SET OF PINS, AND THE ARC DROVE ONE OF THEM

The path chose `pin=0x0a` (`assoc=1 seq=2`) over `pin=0x0b` (`assoc=1 seq=0`). **Why:** the rank in
`walk_codec` is a function of (default device, gross location) only. Both pins are
`dev=speaker loc=internal`, so both score rank 0; `best` is replaced only on a *strictly* smaller
rank; the ascending-NID scan therefore hands the tie to 0x0a because `0x0a < 0x0b`. Association and
sequence were decoded and printed by arc 1 and then never consulted. \[HDA-SPEC §7.3.3.31\] makes
sequence the ordering *within* an association and **sequence 0 its primary member** — the
specification supplies exactly the tie-break the rank was missing, and the rank now uses it.

But a tie-break still drives one pin, and a two-member association **is** the stereo pair; the two
pins have *different* converters (`0x0a conns=[3]`, `0x0b conns=[4]`). So arc 2 now collects every
output-capable pin of the chosen pin's association carrying the same default device and owning a
converter of its own, orders the members by sequence, and binds **all** of them to the same stream
tag. The starting channel per member \[HDA-SPEC §7.3.3.11\] is 0 for a **stereo** converter
(`PARAM_WIDGET_CAPS` bit 0 — both CS4206 converters report `caps=0x000d041d`, bit 0 set, so each
consumes both channels of the two-channel stream; asking the second for channels 1–2 would ask for a
channel the stream does not carry) and the sequence index for a mono one. Every member's binding is
read back on its own `[hda] bind` line and **one member that did not take the binding fails the
whole run**.

#### The second controller, untouched

`[hda] census bdf=1:0.1 id=10de:0e1b … bar0=0xc1080000` is the GK107 Kepler's HDMI audio function.
The driver claims the first controller that resets and answers and stops — it claimed `0:27.0` and
never touched `1:0.1`. That is deliberate and it is **arc 4** (§9).

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
UNAOS_WC=1 UNAOS_HDA=1 UNAOS_HDATONE=1 UNAOS_QEMU_FULL=1 ./arroyo test 120   # HDAAMP's gate
```

The `hda-duplex` codec declares **no GPIOs**, so the HDAAMP arc's GPIO step prints
`[hda] gpio … -> none` there, issues no verb, and the fixture's verdict is unchanged: the whole
defect-2 mechanism is metal-only by construction and the fixture says so on its own line rather than
being silent about it.

### Go-red by mutation — one per clause of the verdict, each measured

| mutation | verdict line measured | leg |
| :--- | :--- | :--- |
| `SDnCBL` written 0 (the cyclic buffer is empty, so the engine has nothing to walk) | `lpib_advanced=0 bcis=0 tag_ok=1 → FAIL` | rc 1 |
| `STREAM_TAG` = 0 (the "unused" tag; the link never carries it) | `lpib_advanced=1 bcis=2 tag_ok=0 → FAIL` | rc 1 |
| *(the same tag mutation, against the FIRST version of the verdict)* | `lpib_advanced=1 bcis=2 → PASS` — **did not go red** | rc 0 |
| **HDAAMP, 2026-09-22 — `SDnCBL` written 0, re-measured against the NEW verdict**, because the verdict's second clause changed (`walked = wraps > 0 \|\| bcis > 0`) and a go-red row measured against the old clause is not evidence about the new one | `lpib=0 -> 0 (max 0) bcis=0 wraps=0 consumed=0 rate_bps=0` -> `:: HDA-TONE: lpib_advanced=0 walked=0 wraps=0 bcis=0 tag_ok=1 fifo_ready=1 run_ms=1200 members=1 -> FAIL ::` — **both** new clauses fail, which is the point: the wrap witness cannot be satisfied by an engine with nothing to walk | rc 1 |

Both live mutations red the leg through `arroyo`'s FAULT-SCAN list and `mbench`'s
`DEFAULT_FORBIDS`; reverting each restores the pass. The third row is kept deliberately: it is the
measurement that produced the `tag_ok` clause, and deleting it would leave a reader unable to tell
why the verdict has three terms instead of two (LAWS §5 — record the defended near-miss as a
counter-example).

### Byte identity

Both knobs are `./arroyo knoboff hda` / `./arroyo knoboff hda-tone`, exit 0 (byte-identical), with
`warm=yes` quoted beside the verdict. The shape that earns it:

- `drivers/hda.rs` is **not lexed at all** knob-off — the `#[cfg]`-erased `pub mod` in
  `drivers/mod.rs` is the one case LAWS §5 names as byte-safe for a module — and the declaration is
  the **last** line of that file, so no module above it moves. **This is what makes HDAAMP's arc-1
  edit safe:** the sequence tie-break (§6, defect 3) is a change *above* the arc-2 banner, i.e. in
  code the `hda` knob arms, and it would move the `hda`-ON image — but `knoboff` compares the
  DEFAULT image, in which this file does not exist, so both knobs still measure exit 0. A reader
  should not take that as licence: an arc-1 edit is only invisible here because the whole module is
  erased, and the same edit in any file the default build lexes would have to be line-neutral.
- `drivers/pci.rs::audio_inventory` is an **impl-tail** append and the probe call is a **line-neutral**
  append on an existing line, before that line's first `//` (LEDGER P7).
- Every arc-2-only constant, the `cmd16` verb form, `elapsed_ms` and the whole `mod tone` block carry
  `#[cfg(feature = "hda-tone")]`, so `hda` alone lexes none of them.
- `arroyo`'s `arm_features` strips both names, so neither knob can shift an aarch64 `-Cmetadata`
  fingerprint. ⚠ `drivers/pci.rs` **is** lexed on aarch64, unlike `drivers/ahci.rs` — which is why
  both of its sites carry `target_arch = "x86_64"` **in the attribute** rather than relying on the
  module gate; the `gen7` entry in `arm_features` records the review that caught exactly that shape.

---

## 6a. The QEMU codec's walk, as measured

`UNAOS_HDA=1 UNAOS_HDATONE=1 UNAOS_QEMU_FULL=1 ./arroyo test 120`, rc 0, MBENCH 6/6 required
witnesses and 0 forbidden hits over 1754 lines, full wall 120.3 s (`mode=full`, `completion_at=-`,
from the capture's own `.run` sidecar). This is the whole `[hda]` wire of that boot, taken at the
tip of this series:

```
[hda] census bdf=0:3.0 id=8086:2668 class=04 sub=03 progif=00 (hd-audio) bar0=0x810c4000 irq=11
[hda] map bdf 0:3.0 bar0=0x810c4000 len=0x2000 uncacheable
[hda] gcap=0x4401 oss=4 iss=4 bss=0 nsdo=0 64ok=1 version=1.0
[hda] reset crst=1 statests=0x0001 codecs=[0]
[hda] rings corb=0x178ab00 rirb=0x178d700 entries=256/256 corbctl=0x02 rirbctl=0x03 rintcnt=1 intctl=0x00000000(untouched)
[hda] ring-state after-first-verb corbwp=0x0001 corbrp=0x0001 rirbwp=0x0001 rirbsts=0x01->0x00 corbctl=0x02 rirbctl=0x03 rintcnt=0x0001
[hda] codec=0 vid=1af4:0022 rev=0x00100101
[hda] codec=0 fg=0x01 type=0x01 (audio)
[hda] node=0x02 type=audio-out conns=[] caps=0x0000001d out-amp=1 in-amp=0 amp-override=1 power-ctl=0 digital=0 pincfg=-
[hda] node=0x03 type=pin conns=[2] pincap=0x00000010 out=1 hp=0 eapd=0 pincfg=0x00004010 (dev=line-out loc=external conn=unknown colour=green portconn=0 assoc=1 seq=0)
[hda] node=0x04 type=audio-in conns=[5] caps=0x0010011b out-amp=0 in-amp=1 amp-override=1 power-ctl=0 digital=0 pincfg=-
[hda] node=0x05 type=pin conns=[] pincap=0x00000020 out=0 hp=0 eapd=0 pincfg=0x00805020 (dev=line-in loc=external conn=unknown colour=red portconn=0 assoc=2 seq=0)
[hda] path codec=0 dac=0x02 -> [3, 2] -> pin=0x03 dev=line-out hops=2
[hda] walk codecs=1 nodes=4 dacs=1 pins=2 speaker_pin=none hp_pin=none path=found
[hda] audit stage=walk wrote-cfg=0 wrote-ctrl=56 wrote-stream=0 verbs-get=17 verbs-set=1 wrote-intctl=0(audited) wrote-wallclk=0(audited) wrote-dplbase=0(audited)
:: HDA: codecs=1 nodes=4 dacs=1 pins=2 path=found -> PASS ::
[hda] tone arm sd=0 (iss=4 => descriptor 4) fmt=0x0011(readback 0x0011) cbl=192000(readback 192000) lvi=1 bdl=0x178af00 pcm=0x1794800 bytes=192000 tag=1 srst=1/1 ctl=0x00100004
[hda] tone stream=0 lpib=0 -> 44356 (max 191964) bcis=2 fifo_ready=1 run_ms=1200 sts=0x20 fifoe=0 dese=0 cbl=192000 tag=1 tag_bound=0x10 tag_ok=1
:: HDA-TONE: lpib_advanced=1 bcis=2 tag_ok=1 fifo_ready=1 run_ms=1200 -> PASS ::
[hda] audit stage=tone wrote-cfg=0 wrote-ctrl=84 wrote-stream=24 verbs-get=23 verbs-set=9 wrote-intctl=0(audited) wrote-wallclk=0(audited) wrote-dplbase=0(audited)
[hda] audit stage=end wrote-cfg=0 wrote-ctrl=86 wrote-stream=24 verbs-get=23 verbs-set=9 wrote-intctl=0(audited) wrote-wallclk=0(audited) wrote-dplbase=0(audited)
```

What is worth reading off it, because it is what the metal boot will be compared against:

- **`oss=4 iss=4`**, so output stream 0's descriptor is number 4, at `0x80 + 4 × 0x20`. The arm line
  prints the arithmetic (`sd=0 (iss=4 => descriptor 4)`) rather than leaving a reader to do it.
- **`vid=1af4:0022`** — the QEMU codec, and the only codec id this project has ever read. The rMBP's
  is still unread; §7.
- **Four widgets, two pins**, and `speaker_pin=none`: this codec has a line-out and a line-in and no
  speaker at all, which is why §2.6's rank is over output devices rather than a speaker-only match.
  **The metal boot is the first time the speaker rank is exercised on anything.**
- **`wrote-cfg=0`** — the fixture's firmware had already set memory decode and bus master, so the
  claim needed no config write at all on this machine. On the rMBP this may read 1.
- **The LPIB values are not reproducible to the byte and must not be pinned in a spec.** 1200 ms of
  a 1000 ms cyclic buffer wraps once and lands wherever the host's scheduling put it: three runs of
  this same tip read `44160`, `44180` and `44356`. What is stable, and what the verdict is computed
  from, is that it MOVED and that `bcis` reached 2.
- **`[hda] node=0x03 … conns=[2]`** — the connection list is one entry, so the derived path is the
  shortest one that exists (`dac=0x02 -> pin=0x03`, two hops) and neither the mixer nor the selector
  branch of the path programming is exercised here. Those are metal-first by construction.

---

## 6b. HDASIE — the INTCTL.SIE latch experiment (rmbp-ledger B207, 2026-09-24)

Flight 11 (§6) read `bcis=0` on a stream that walked the whole cyclic buffer at the link rate with
IOC set in both BDL entries and `SDnCTL.IOCE` set. B130 changed the verdict so that a wrap is the
second witness and left one question open: **why does the 7-series PCH (`8086:1e20`) not latch
`SDnSTS.BCIS`?** The one precedent in this file is §2.4: `RIRBCTL.RINTCTL` gates the `RIRBSTS.RINTFL`
*latch*, not only the interrupt. The symmetric candidate is `INTCTL.SIE[n]` [HDA-SPEC §3.3.14] gating
the `SDnSTS.BCIS` latch. `INTCTL` was this driver's audited never-written register, so the experiment
is its own knob and changes exactly one thing.

**Knob.** `UNAOS_HDASIE=1` → feature `hda-sie` (implies `hda-tone`). Wired in `arroyo` (mapping and
`arm_features` strip), `builder/src/main.rs` (the media list) and `Cargo.toml`.

**What it does, and what it does not.** `sie_arm` runs on the `let lpib0` line, immediately before
RUN: reads `INTCTL`, sets bit `n` for the ONE descriptor the tone runs on (`iss`, the first output
descriptor), reads back. `sie_restore` runs on the `a.stream += 9` line after STOP, reset and the
register restores: writes the value read before the arm, reads back. **`GIE` (bit 31) and `CIE` (bit
30) are never written**, so no interrupt can reach the CPU either way — this is a latch experiment,
not an interrupt arc, and `[hda] rings … intctl=…(untouched)` stays true where it prints (before the
tone). The audit line's `wrote-intctl=0(audited)` is REPLACED with the knob on by a file-tail
`impl Audit` that prints the count: `wrote-intctl=2(sie)`. An audited zero is never printed over a
register this driver has written.

**Wire.**
```
[hda] intctl sie desc=N bit=0x… before=0x… want=0x… after=0x… set=1 gie=0 cie=0
[hda] tone stream=0 … bcis=N …                      ← THE MEASUREMENT
[hda] intctl restore desc=N armed=0x… saved=0x… after=0x… restored=1 bcis_with_sie=N
:: HDA-SIE: desc=N sie_set=1 restored=1 bcis=N -> PASS ::
[hda] audit stage=tone … wrote-intctl=2(sie) …
```
**The verdict is about the restore, on purpose.** `-> PASS` says the register was put back to the
value read before the arm. `bcis=` is the finding and is not scored here: on the bench, `bcis>0`
with SIE set and `bcis=0` without it (flight 11) *is* the answer "SIE gates the latch"; `bcis=0` with
`sie_set=1` says the hypothesis is wrong and the next candidate is the descriptor's own `IOCE`
polarity or a controller quirk; `sie_set=0` says the bit would not take and is its own finding. None
of those is a defect of this code, so none of them reds the run under DEFAULT_FORBIDS.

**QEMU.** `hda-duplex` latches BCIS with or without SIE, so the fixture proves the knob's mechanics
(set, readback, restore, audit count, tone verdict unchanged) and cannot answer the question. The
gate line and the go-red are in `docs/dev/evidence/rmbp-0924/hdasie/HDASIE.md`.

**Byte identity.** Both call sites are same-line and cfg-gated, both functions and the audit variant
are file-tail; knob-off the module is not lexed at all (the "Byte identity" note above), so
`./arroyo knoboff hda-sie` holds by the same argument as `hda-tone`.

**Flight line** (the metal answer; one boot, the ear optional): flight 12's knobs plus
`UNAOS_HDA=1 UNAOS_HDATONE=1 UNAOS_HDASIE=1`. Score `bcis=` on the tone line against flight 11's `bcis=0`.

## 7. The metal expectation

The bench machine is a 2012 15" Retina MacBook Pro, MacBookPro10,1, Intel 7-series (Panther Point)
PCH. **Expected, not assumed** — the census prints what is there. **Flight 11 (2026-09-22) settled
every row of this table; the `measured` column is the wire, not a prediction** (§6):

| fact | expectation | measured, flight 11 |
| :--- | :--- | :--- |
| controller bdf | `0:27.0` | ✅ `0:27.0`, and a **second** class-0x04 function at `1:0.1` |
| controller id | `8086:1e20` (7-series HD Audio) | ✅ `8086:1e20`; the second is `10de:0e1b` (GK107 HDMI audio, §9) |
| class triple | `class=04 sub=03 progif=00` | ✅ both functions |
| codec | Cirrus Logic **CS4206** — vendor 0x1013 | ✅ `vid=1013:4206 rev=0x00100302` |
| codec count | 1 | ✅ `statests=0x0001 codecs=[0]` |
| internal speaker pin | `dev=speaker loc=internal`, EAPD-capable | ⚠ **TWO** of them — 0x0a (`assoc=1 seq=2`) and 0x0b (`assoc=1 seq=0`) — and **EAPD-capable is FALSE**: `eapd=0` on both, `PIN_CAPS` bit 16 clear (defect 2, §6) |
| headphone pin | `dev=hp-out loc=external` | ✅ `hp_pin=0x09`, `pincfg=0x002b4020 (… assoc=2 seq=0)` |
| the tone | `lpib` advancing, `bcis` ≥ 2, `tag_ok=1`, and **Peter hears 440 Hz from the internal speakers** | ⚠ `tag_ok=1`, LPIB walked the **whole** buffer and wrapped at 191997 B/s — and `bcis=0`, and **the room was silent** (§6) |

The codec's vendor/device id was the one line of §7 nobody in this tree had ever read; it now reads
`1013:4206`. A discrete GPU's HDMI audio function may also appear in the census on this machine —
the GK107 has one, and flight 11 printed it — which is why the census prints every class-0x04
function and the driver claims the first that resets and answers. Driving two controllers at once is
**arc 4** (§9) and is deliberately not done quietly here.

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
   that answers is claimed. The *second controller* on the bench machine has a section of its own —
   §9, arc 4 — now that flight 11 has measured it.

---

## 9. Arc 4 — the second controller, `1:0.1` `10de:0e1b`

Flight 11's census printed **two** class-0x04 functions, and only the first was touched:

```
[hda] census bdf=0:27.0 id=8086:1e20 class=04 sub=03 progif=00 (hd-audio) bar0=0xc1c10000 irq=0
[hda] census bdf=1:0.1 id=10de:0e1b class=04 sub=03 progif=00 (hd-audio) bar0=0xc1080000 irq=0
```

`10de:0e1b` is the **GK107 Kepler's HDMI audio function** — bus 1, device 0, function 1, beside the
GPU itself at `1:0.0`. It is the same HD Audio controller architecture (CORB/RIRB, codec verbs,
stream descriptors), with a codec whose pins are HDMI/DisplayPort sinks rather than speakers, so
nothing in arcs 1–3 would have to be rewritten for it: the walk, the path derivation and the stream
programming are the same code and the difference is which pin the path lands on.

**It is untouched, and that is deliberate.** `probe` claims the FIRST controller that resets and
answers and then returns — one controller per boot — so on this machine the PCH's analog controller
wins and the Kepler's is enumerated, printed and left alone. Claiming both means two `Rings`, two
widget tables and a decision about which one a tone should come out of, and it interacts with the
GPU's own bring-up (the Kepler is the subject of an entire ladder of its own; `docs/dev/OS/08_VIDEO/`).
That is an arc, not an adjacent improvement.

What arc 4 owes, in order: claim the second function under a knob of its own; walk it and print its
codec id and pin default configurations (an HDMI pin's `dev=` is `digital-other-out` or `spdif-out`,
and its default configuration carries the connector rather than a speaker); decide the
one-controller-per-boot rule properly rather than by "first that answers"; and only then consider a
tone, which on an HDMI sink means a display that is awake and a link that is up — i.e. it depends on
the Kepler ladder, not on this file.
