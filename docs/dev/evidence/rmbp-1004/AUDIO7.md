# AUDIO7 — one stream discipline for the tone and the player (rmbp-ledger B313)

Branch `exec-rmbp-audio7`, cut from 3160b02a. Answers FLIGHT19 §2 (HDATONE7 + PLAY2 + `tests playwav`).
Seam: `CHARTER: Stria — owed B289`. The kernel player stands in for Stria (the A/V handler, host-only on
cpal, no media `SMessage`); nothing here is a second Stria — it is the HDA driver's stream half, the part a
Stria fulfiller would call. The fulfiller registration that would let Stria own `play` is BANDY3's (B301).

## 1. The finding, read from the log (f19-boots.log, awk on `[hda]`, `[play]`, `:: HDA`, `:: PLAYWAV`)

**Tone (HDATONE7).** 18 `:: HDA-TONE:` lines across the boots: 17 PASS, 1 FAIL (`tests hda2`, 15:33:33Z).
The 18 `[hda] tone arm` lines are BYTE-IDENTICAL once the two heap addresses are cut out
(`fmt=0x0011(readback 0x0011) cbl=192000(readback 192000) lvi=1(readback 1) tag=1 srst=1/1 ctl=0x00140004 ioce=1`,
`uniq -c` = 18). Every register the driver wrote and read back is the same on every run. What differs run
to run:

| field | values over 18 runs | reading |
| :--- | :--- | :--- |
| `pcm=`/`bdl=` | a fresh heap address every run (`dma_alloc`, never freed); BDL slots reused (`0x2034c480..0x2034c680`) | the DATA the DMA reads is the one thing that is new each run |
| `bcis=` | 0 on 14 runs, 2 on 3 (+ the FAIL) — with an identical LPIB walk (`0 -> 38400`, `wraps=1`, `rate_bps=192000`) | the IOC latch is not deterministic either |
| `pin_pwr=` | `D?` (never read) on 10, `D0` on 8 | the witness is incomplete, not the hardware |
| `settled_ms=` | 5 or 6 | noise |
| the FAIL | `lpib=0 -> 0 fifo_ready=0 sts=0x00` with `run_bit=1`, three stall reasserts, no DESE/FIFOE | the controller never fetched a byte: a BDL it could not use, with no error |

**So the stream REGISTERS are not what varies** — `tests hda` resets the whole controller (CRST, which also
resets the codec over the link) at the top of every run, so no descriptor or converter state survives from
the previous run. That retires most of the brief's stale-state list (stale LPIB, BDL not reset, leftover
tag) for the TONE path, and M1 below prints the proof per run instead of inferring it.

**What the evidence does point at: DMA coherence.** The PCM buffer and the BDL are written by the CPU into
write-back heap memory and never flushed (`hda.rs` `issue()` comment: "DMA is snooped, so this is a
compiler/store ordering fence, not a cache flush" — an assumption, never read). If the controller's stream
DMA is not snooped (PCIe Device Control "Enable No Snoop", bit 11 at cap+8 — 0x78 on this PCH, which Linux
`hda_intel` CLEARS on every Intel PCH, `AZX_DCAPS_SNOOP_TYPE(SCH)`; and Linux also clears TCSEL 0x44[2:0]
"to clear playback static"), the DMA reads whatever DRAM held: part of the new sine (lines already
evicted), part of the PREVIOUS run's buffer, part stale heap. That is exactly Peter's ear — "random …
mostly screeching with some silence and a tone here and there" — and it explains the one run that never
fetched (a stale BDL in a reused slot pointing at nothing usable). It also explains why the GPIO amp arm
(HDATONE6) changed nothing: the amp was never the variable.

**The 0x4011 / 0x0011 pair.** Intel HDA stream format (SDxFMT and the converter format verb share the
layout, HDA spec §3.3.41): bit 15 type (0 = PCM), bit 14 BASE (0 = 48 kHz, 1 = 44.1 kHz), bits 13:11 MULT
(x1..x4), bits 10:8 DIV (/1../8), bits 6:4 BITS (0 = 8, 1 = 16, 2 = 20, 3 = 24, 4 = 32), bits 3:0
CHAN (channels - 1). `0x0011` = PCM, 48 kHz x1 /1, 16-bit, 2 ch. `0x4011` = the same with BASE = 44.1 kHz.
The link clock is 48 kHz-based; a 44.1 stream is carried by the controller's sample-rate cadence, and a
converter set to one base fed by a descriptor set to the other plays at the wrong rate and slips frames
(a screech). In the log the two NEVER disagree within a run (`sdfmt=0x0011 conv=0x0011 match=1` on every
tone; play `fmt=0x4011(readback 0x4011)` with the descriptor written the same word) — and the tone runs
after a CRST, so play's 0x4011 cannot leak into a later tone. M2 makes the match a checked invariant
(`dac_fmt_match=`) instead of a coincidence.

**A witness defect found reading the code.** Every `[hda] amp … raw=0x0000 gain=0` line read the WRONG
amplifier: `rings.cmd(.., VERB_GET_AMP_GAIN_MUTE /*0xB00*/, 0x80)` is verb B payload `0x0080`, i.e. INPUT
amp, RIGHT channel, index 0 — a DAC has no input amp, so it answers 0. Get Amp Gain/Mute payload: bit 15
= output, bit 13 = left. The run's actual DAC gain was never read. Fixed (payload `0xA000` / `0x8000`,
16-bit form) in the run snapshot and in the three existing reads.

**Play (PLAY2) — silent because it never ran.** `hda_play.rs` starts the stream only once its FIFO holds
a whole ring (`RING_BYTES` = 128 KiB), but `feed()` refuses a chunk when `fifo + frames * ratio * 4 >
FIFO_CAP` (128 KiB) with `ratio = eff/src + 1` = 2 at equal rates — a 2x over-estimate. The FIFO tops out
at 96 KiB (48 kHz TEST.WAV: 32+32+32 KiB, the fourth chunk refused) or 90312 bytes (the 44.1 kHz file:
3 x 30104) and stays there: never full, never `ended` (the producer is blocked), never RUN. Armed and
silent, every time; `tests playwav` times out at its 8 s wall. That is a deadlock, not a codec question —
and it would have hidden the coherence defect too, so play gets the same flush.

## 2. Milestones

- **M1 — register truth per run.** `hda::stream::snapshot` reads SDxCTL (run / srst / stripe / tag),
  SDxSTS, SDxLPIB, SDxCBL, SDxLVI, SDxFMT, SDxBDPL/U; per member the DAC converter format (verb A),
  stream/channel (F06), DAC out amp L/R (verb B, payload A000/8000), DAC power (F05), pin ctl (F07), pin
  out amp, EAPD (F0C), pin power; the AFG power and GPIO data/dir/enable; PCI DevCtl No-Snoop and TCSEL;
  a CPU-side checksum of the first 4 KiB of the buffer. ONE line `[hda] run=<n> <who> pre: …` before the
  rearm, ONE line `… post: …` after, and `[hda] run=<n> diff vs run=<m>: pre=[…] post=[…] stable=<0|1>`
  against the previous run of the same kind (heap addresses, LPIB and the checksum excluded — they are
  meant to differ).
- **M2 — `hda::stream::rearm(&Params)`**, the one reset both tone and play call: clear RUN and wait for
  it, SRST pulse with both waits, STS W1C, flush the buffer + BDL to memory (`clflush` + `mfence`), allow
  snooping (DevCtl bit 11 cleared, TCSEL to TC0 — both read back and printed), rewrite BDPL/U, CBL, LVI,
  FMT, tag (stripe 0); on the codec: D0 on every path node, connection select, stream/channel then the
  converter format = the SAME word as SDxFMT, out amps unmuted at the codec's declared 0 dB step, pin OUT
  enable (+HP drive where capable), EAPD where capable; then read back and score `dac_fmt_match`,
  `tag_match`. `stream::run` sets RUN and reads it back. The tone's own arm block and play's `gate` arm
  block are replaced by the call (hda.rs edits are line-neutral).
- **M3 — PLAY2.** The FIFO deadlock fixed (`ratio` = ceil(eff/src), `FIFO_CAP` = 2 x ring, and start
  whenever another chunk could not fit). The ring entries are flushed after each refill. RUN read back at
  start; LPIB printed every 100 ms for the first second then every second; witness
  `[play] run=1 lpib=<moving> tag=<match> dac_fmt=<match> level=<rms of first buffer>`.
- **M4 — the fixtures.** `tests hda` ends with `:: HDA: runs=<n> fields_stable=<0|1> dac_fmt_match=1
  tag_match=1 lpib_moved=1 -> PASS ::`. `tests playwav` completes on the ring's own drain (LPIB past the
  last data entry, then all four entries refilled with silence) — a STALL guard (LPIB still for 1 s, or not
  running 2 s after arm) is the failure, not a wall clock — and SKIPs when no codec arms:
  `:: PLAYWAV: path= rate= frames= lpib_moved=1 done=1 -> PASS ::`. R80: nothing runs at boot; both are
  `tests` verbs only.

## 3. Witness — what boot 20 prints

```
[hda] run=1 tone pre: sdctl=0x000000 run=0 srst=0 stripe=0 tag=0 sts=0x00 lpib=0 cbl=0 lvi=0 fmt=0x0000 bdl=0x0 ; m0 dac=0x04 conv=… sc=0x00 dacamp=…/… dpwr=D3 pin=0x0b pinctl=0x00 pamp=… eapd=0x00 ppwr=D0 ; afg=D0 gpio=0x08/0x0a/0x0a ; nosnoop=<0|1> tcsel=<n> ; sum=0x…
[hda] rearm who=tone sd=4 stop=1 srst=1/1 flushed=<bytes> nosnoop=<before>-><after> tcsel=<before>-><after> fmt=0x0011 sdfmt_rd=0x0011 tag_rd=1 dac_fmt=[0x0011] sc=[0x10] dac_fmt_match=1 tag_match=1
[hda] run=1 tone post: … run=0 tag=1 … fmt=0x0011 … conv=0x0011 sc=0x10 …
[hda] run=2 diff vs run=1: pre=[] post=[] stable=1
:: HDA: runs=2 fields_stable=1 dac_fmt_match=1 tag_match=1 lpib_moved=1 -> PASS ::
[play] run=1 lpib=1 lpib0=0 lpib_now=<n> tag=1 dac_fmt=1 level=<rms>
[play] t=100 lpib=<n> …
:: PLAYWAV: path=/home/una/TEST.WAV rate=48000 frames=96000 lpib_moved=1 done=1 -> PASS :: …
```

## 4. Metal read list for boot 20

1. `tests hda` three times. Ear: is it the SAME sound three times (stable even if wrong)? Serial: the
   `nosnoop=` / `tcsel=` values on the `rearm` line (were they set by firmware — the coherence finding),
   `diff … stable=1`, `dac_fmt_match=1`, the DAC amp L/R words now read from the right amplifier.
2. `play /file_example_WAV_1MG.wav` (and `tests playwav`). Ear: any sound at all. Serial: `[play] run=1
   lpib=1`, the `level=` (non-zero), LPIB moving on the `t=` lines, `done=1`.
3. If the tone is now clean and stable: the coherence finding is confirmed — record which of the two
   (flush or snoop bit) mattered by the `nosnoop=` before-value. If it still screeches identically each
   run: the data path is ruled out and the next suspects are the CS4206 vendor coefficient init (node
   0x11, Linux `patch_cirrus.c` errata verbs — owed, not written: no legal source in tree) and the DAC
   gain step now printed correctly.

## 5. Owed

- Stria owns `play` (B289 / BANDY3 fulfiller). The kernel player stays the stand-in until then.
- CS4206 vendor coefficient init (node 0x11) — not written; needs a citable source.
- The tone path keeps its own save/restore of codec registers around the run; with a CRST every run it is
  redundant, and a later arc can drop it once boot 20 confirms the rearm.
