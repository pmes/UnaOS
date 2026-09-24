# FLIGHT 14 — image 7 (rmbp12flight14, hw-rmbp@0614a9fe), 2026-09-24, a cafe

Capture: `f14-boot1.log` (12877 lines, 0 → 577 s; the slice from byte 7092198 of the flight-8 log).
Image: gate14s (artifact leg + certs on the bench; the QEMU proof the cloud session's), banner-cert
ok=48, knob line = flight 12's, tone at the 4096 default (R66).

## Peter, verbatim (the glass)

- "no pw alert came up"
- "mousing is great"
- "select copy and paste works!"
- "no word wrap in terminal. we got rid of the umv and all that i thought? i did a mv to /test not sure if the filename is long enough"
- "vug did much much better all six opened and started working almost instantly"
- "sound came through really screeching again"
- "tried logout and it did nothing shut down works good still"
- "i cannot click and hold with one finger and drag with the other but doing it one handed works good"

## 1. THE SITTING VOIDED AGAIN — readiness is fixed, the mount is not (mechanism established; LOGIN16)

- LOGIN15 (B213) did its half: `[7121ms] :: USERSREADY: rmbp-shape(global=0 sdhc=1 ahci=1)=1 … this-boot
  ready-by=global=0 sdhc=1 ahci=1 -> PASS ::` — the service's guard now opens on the card.
- Then `try_load()` calls `fs::fat::mount()` = `mount_source(BlockSource::Default)`, and `Default` is
  `crate::drivers::block::info()` — the GLOBAL slot, which the rMBP never sets (flight 13 §1). `fat.rs`'s
  doc on `mount()` says "registered, else (x86 + `sdhcblk`) the card in the machine's internal SD slot";
  the code does only the first clause. So every pass is `NoDisk`, and the retry bound closes it:
  `[61354ms] [users] el0-fat volume did not mount after 4096 passes — last=NoDisk — store unavailable
  this boot`. No `[users] load`, no `[rand]`, no root row, no alert, no `[login]` line but the boot one.
- Consequence on the glass: Log Out "did nothing" — `[557819ms] :: SHARD: log out — the session closes and
  the login screen returns ::` then `[users] logout REFUSED session=root reason=storage-not-up (R63:
  adduser first …)`. Correct refusal, wrong reason for Peter (the store was never up), and SILENT on the
  glass — a refused Log Out should say so where he is looking (LOGOUTUI, small).
- Fix (LOGIN16): resolve `BlockSource::Default` (or `try_load`'s mount) the way `block::program_source()`
  already does — the global, else the SDHC card, else the AHCI registry — with USERSREADY's three shapes
  as the go-red (the rMBP shape must MOUNT, not merely be ready). The `mount()` doc string is the spec
  the code must meet.

## 2. Flown green

- **TPSCALE (B214)**: "mousing is great". 56 `[tp] mt fingers=1 … div=8` witnesses, max |dx| 16, max
  |dy| 16 per witness frame (flight 13: 88/60 at 1:1). Clicks: `trackpad click (button-down edge,
  buttons=0x01)`. Y sign: not reported inverted.
- **IOAPIC3 (B216, R68)**: `[5836ms] [ioapic] pirq bdf=0:31.0 id=8086:1e57 … pirqg_rout=0x80 irqen=1
  … d29ir=0x3236 -> gsi=22 via=apic-input`, `[ioapic] route bdf=0:29.0 pin=INTA line=0 -> gsi=22 via=pirq
  polarity=active-low trigger=level`, `:: EHCI-HID: [1] ISRARM armed via=ioapic-intx gsi=22 vector 0x43`;
  then `[39561ms] :: EHCI-HID: ISRARM armed=1 refused=0 irq=38 isr_rearm=5 poll_rearm=0 depth_max=4` —
  THE FIRST EHCI INTERRUPT DELIVERED ON THE rMBP (flights 8-13: polled, refused). No `ISRARM IRQ DEAD`.
- **TERMSEL2 (B196)**: "select copy and paste works!" — `[clip] paste bytes=22` at 172846 ms; the pasted
  fragment of `help` carried a newline, which the typed path DISPATCHED (`[midden] cmd="port][/path]
  (HTTP/1." -> TerminalError`) — the documented behaviour (clipboard.md §3, "select within one line").
- **HDATONE2 (B215)**: `:: HDA-PCM: amp=4096 default=4096 peak=4095 min=-4096 q27=4095 interleave=1
  peak_ok=1 sine_ok=1 le_ok=1 frames=48000 -> PASS ::`, `:: HDA-TONE: … -> PASS :: amp=4096 ::` — the
  instrument flew; what it excludes is in §3.
- **LOGIN15 (B213)**: the predicate, as above. **BOOTSLOW/GLASSFIX3**: as flight 13 (not re-read).
- Shutdown: "shut down works good still".

## 3. Sound: "really screeching again" at 4096 — the level and the buffer are now EXCLUDED

Flight 13 at 12288: sandpaper. Flight 14 at 4096, with the PCM buffer read back as a clean, interleaved,
little-endian sine at the right peak, and the DMA at exactly 192000 B/s: screeching. So it is not the
amplitude, not the sample layout, not the DMA rate. Left: the codec side — `[hda] amp member=0 dac=0x04
raw=0x0000 gain=0 pin=0x0b … out_amp=0 pinctl=0x40 out_en=1 hp_en=0 fmt_conv=0x0011 fmt_want=0x0011
fmt_match=1` — a converter FORMAT the codec accepted but may not run at 48 kHz (a "screech" is pitch, not
noise: a 440 Hz sine played at the wrong converter rate is a screech), the two DACs 0x04/0x03 driving pins
0x0b/0x0a in `members=2` (both speakers, or one speaker and the headphone path?), or the class-D amp's
GPIO (HDAAMP defect 2). **HDATONE3**: read back the converter's stream format and rate after
`SET_CONVERTER_FORMAT`; try one member; a 220 Hz / 2 s tone so the ear can name the pitch; the
`[hda] amp` line gains `rate=`.

## 4. New readings

- **TPDRAG**: "i cannot click and hold with one finger and drag with the other but doing it one handed
  works good". Every one of the 56 mt witnesses reads `fingers=1`; no `fingers=2` frame was witnessed
  in 577 s although Peter had two fingers down. `mt_step` takes finger 0's absolute position; with a
  second finger the frame's finger count, or which finger is 0, is not what the pad sends — the
  `wsp2_parse` finger-count field is the first thing to read against a two-finger frame on the wire
  (capture the raw 58 bytes of one such frame: the `first_bytes` hook exists).
- **TERMWRAP**: "no word wrap in terminal" — Peter typed a ~200-character `mv` line; the edit line does
  not wrap on the shell window. New arc (the console's edit line and scrollback wrap at the window
  width; TERMSEL2's column arithmetic must follow).
- **umv, mv, LFNMV (B202)**: `[176ms] :: [relics] umv -> mv :: retired=true registered=true ::` — `umv`
  IS retired: Peter's `umv hello.txt test/<long>` answered `TerminalError len=44` (the relic's pointer to
  `mv`), then `mv hello.txt test/<~200 chars>` ran as `Host verb=mv`. No LFNMV wire line printed and the
  `ls test` output is not on the wire, so whether the long name landed is UNREAD. The cloud reads
  `mv`'s result path for the witness B202 promised, and why it did not print.
- **VUGART (B162)**: glass "did much much better, all six opened and started working almost instantly";
  wire: every `:: VUGART:` line FAIL (`frames=3265 coherent=3230 torn_rows=4104 mixed_frames=35 -> FAIL`,
  and 20+ smaller ones) — the fixture counts tears the eye does not see at this rate, or the threshold
  is the flight-11 "abstract art" one and too strict now. **VUGART2**: state the threshold against this
  wire, and whether `mixed_frames` at 1% is a tear or a barrier miss.
- **LOGOUTUI**: §1 — a refused Log Out says nothing on the glass.
- **KVBLANK3**: `vector close head=0 irq=1 vbl_delta=62 … mode=irq deliver=msi` again (KVBLANK4 owed, unchanged).

## 5. Owed to the queue from this flight

LOGIN16 first (§1; the sitting cannot start without it), then HDATONE3 (§3), TPDRAG, TERMWRAP, the
LFNMV read, VUGART2, LOGOUTUI (§4). Flown green: B214 TPSCALE, B216 IOAPIC3 (first interrupt), B196
TERMSEL2, B215 HDATONE2 (instrument), B213 LOGIN15 (predicate).
