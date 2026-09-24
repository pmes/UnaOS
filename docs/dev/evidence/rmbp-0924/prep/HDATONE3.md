# HDATONE3 — prep

## The finding

Flight line: `:: HDA-PCM: amp=4096 default=4096 peak=4094 min=-4094 q27=4090 interleave=1 peak_ok=1 sine_ok=1 le_ok=1 frames=48000 -> PASS ::`
(paired with `:: HDA-TONE: … members=2 -> PASS :: amp=4096 ::`, DMA `rate_bps=192000`).

Peter's words: at 12288 it is "sandpaper"; at 4096 it is "really screeching"; and at 4096 it is
"frightening because it is so abrupt … like an old dial up modem." HDATONE2 already excludes
amplitude (amp is on the wire and in the buffer, both PASS at every level tried), sample layout
(`interleave_ok`/`le_ok`/`sine_ok` all PASS), and DMA rate (`rate_bps=192000` matches
`expect_bps=192000`). A squeal that survives a proven-clean buffer at every amplitude is a PITCH
defect, not a level or layout one — the candidates left are: (a) the codec playing back at the
wrong rate despite `fmt_conv==fmt_want` at write time, (b) the raw 0 ms edge of the buffer read as
a click/pop train, and (c) two DACs on one stream tag beating against each other.

## Mechanism

- `unaos/crates/kernel/src/drivers/hda.rs:1391-1403` `tone::fill` — writes the Q15 sine straight in,
  frame 0 to frame FRAMES-1, no envelope. The buffer starts and ends at a hard `0 -> ±peak -> 0`
  edge with zero slew, which is exactly a click/step at t=0 and t=1200ms (`RUN_MS`).
- `unaos/crates/kernel/src/drivers/hda.rs:1413-1432` `tone::check` — samples only frame 0 (must be
  0) and frame 27 (quarter period, must be within 3% of `AMPLITUDE`). Any envelope applied in `fill`
  must be reflected here or `sine_ok` reds at go-green, because frame 27 sits inside a 20ms (=960
  frame) fade window at 48 kHz.
- `unaos/crates/kernel/src/drivers/hda.rs:1840` `VERB_SET_CONVERTER_FORMAT` writes `FMT_48K_16_STEREO`
  (`0x0011`) to each member DAC; `:1896-1906` reads it straight back with `VERB_GET_CONVERTER_FORMAT`
  (`0xA00`) into `fmt_conv`, already compared to `fmt_want` — but the read-back is the FORMAT
  REGISTER, never the codec's declared supported-rates parameter, so a converter that silently
  clamped 48 kHz to a supported-but-different rate cannot be caught by this line today. `PARAM_*`
  consts live at `hda.rs:161-167` and `hda.rs:1275-1297`; none of them is the "Supported PCM Size,
  Rates" parameter (HDA-SPEC §7.3.4.7, id `0x0A`) — it does not exist in this file yet.
- `unaos/crates/kernel/src/drivers/hda.rs:1520-1560` pair-walk — an association with 2+ output pins
  of the same device/assoc (here pin 0x0a seq=2, pin 0x0b seq=0) is driven on BOTH DACs (0x03, 0x04)
  under the SAME `STREAM_TAG`, printed as `members=2` at `:2089-2097`. Two converters on one tag but
  not verified phase/rate-identical is the third candidate: a rate mismatch between the DACs would
  beat, reading as exactly what Peter described.
- `unaos/crates/kernel/src/drivers/hda.rs:1340-1351` `parse_amp`/`AMPLITUDE` is the existing
  build-time-knob idiom (`option_env!`, `const fn`, safe default on bad input) — the model for every
  new knob below. `unaos/arroyo:2666-2671` documents `UNAOS_HDA_AMP`; a new knob's comment goes
  directly under it, same style. Per the brief: do not edit `arroyo` this round, draft text only.

## Plan

**M1 — read back rate, not just format.** Files: `hda.rs` near `:1275-1297` (add
`pub const PARAM_SUPPORTED_PCM: u32 = 0x0A;`) and `:1896-1906` (the `[hda] amp` print). Add a
`VERB_GET_PARAMETER(mdac, PARAM_SUPPORTED_PCM)` call and decode `fmt_conv`'s base/mult/div bits into
a printed `rate=<hz>`; keep the raw hex as `fmt_rd=`, distinct from `fmt_want`. Witness: extend
`[hda] amp member=… fmt_conv=0x.... fmt_want=0x.... fmt_match=.` with ` rate=48000 fmt_rd=0x....
pcmcaps=0x........`. Go-red: swap the decode's mult/div fields — a codec running the flown format
still prints a wrong `rate=`.

**M2 — 20ms linear fade, masked from `check`.** Files: `hda.rs:1391-1403` (`fill`) and `:1413-1432`
(`check`). Add `const FADE_FRAMES: usize = SAMPLE_RATE / 50;` (20 ms) and an `envelope(f)` helper
(ramp 0..1 over the first/last `FADE_FRAMES`) applied to the sample in `fill`. In `check`, replace
the bare frame-27 compare with the same enveloped reference — frame 27 sits inside the fade window
and must not be scored as if it weren't. Witness: `:: HDA-PCM: … fade_ms=20 edge0=0 edge_last=0 ->
PASS ::`. Go-red: envelope `fill` but leave `check`'s frame-27 compare bare — `sine_ok=0` on an
otherwise-correct buffer, proving the mask is load-bearing.

**M3 — 220 Hz / 2s option so the ear can name the octave.** Files: `hda.rs:1331-1332` (`TONE_HZ`,
`FRAMES`/`SAMPLE_RATE`) and the `arroyo` comment block after `:2667` (draft text only, no edit). Add
`pub const TONE_HZ: usize = parse_hz(option_env!("UNAOS_HDA_HZ"));`, allow-listing only `220`/`440`
(never an arbitrary alias of the sample rate), default 440; plus `UNAOS_HDA_SECS` (allow-list
`1`/`2`) scaling `FRAMES`/`PCM_BYTES`/`BDL_*`. Witness: `[hda] tone … hz=220 secs=2` on the existing
tone line. Go-red: `UNAOS_HDA_HZ=300` silently falls back to 440 — proves the allow-list.

**M4 — one-member knob.** File: `hda.rs:1540-1560` (pair-walk loop) — two DACs 0x04/0x03 drive pins
0x0b/0x0a (`members=2`). Add `pub const FORCE_ONE_MEMBER: bool =
option_env!("UNAOS_HDA_MEMBERS").is_some();` (presence-only, mirroring `hda-sie`'s implies-pattern)
gating the pair-walk loop so `np` stays 1. Witness: `members=1` under the knob, `members=2` without.
Go-red: set the const but still enter the loop — `members=2` prints regardless of the knob.

## Spec pins

No `unaos/scripts/specs/*.spec` file exists for this arc yet (none of the 19 files under
`scripts/specs/` mention `hda`). A new `scripts/specs/x86-hdatone3.spec`, once M1-M4 land, pins:

```
REQUIRE :: HDA-PCM: amp=\d+ default=\d+ peak=-?\d+ min=-?\d+ q27=-?\d+ interleave=1 peak_ok=1 sine_ok=1 le_ok=1 fade_ms=20 edge0=0 edge_last=0 frames=\d+ -> PASS ::
FORBID :: HDA-PCM: .* -> FAIL
REQUIRE \[hda\] amp member=\d+ dac=0x[0-9a-f]+ .* fmt_conv=0x[0-9a-f]+ fmt_want=0x[0-9a-f]+ fmt_match=1 rate=48000 fmt_rd=0x[0-9a-f]+ pcmcaps=0x[0-9a-f]+
FORBID \[hda\] amp .* fmt_match=0
REQUIRE :: HDA-TONE: lpib_advanced=1 walked=1 .* members=(1|2) -> PASS :: amp=\d+ ::
FORBID :: HDA-TONE: .* -> FAIL
```

No look-around; every pin is a literal-anchored `REQUIRE`/`FORBID` on a named field (LAWS §5: require
a property, never a limitation).

## Open questions

- Which of M1/M2/M4 to fly first is a one-box, one-afternoon budget call — Peter should say whether
  the rate readback (M1, cheapest, purely diagnostic) or the fade (M2, most likely to kill the
  "abrupt … dial-up" description on its own) goes first.
- M3's octave-naming tone (220 Hz) is a diagnostic aid, not a fix — confirm it's wanted as a
  standing knob vs. a one-off manual edit for the next flight.
- M4 assumes the beat-note theory is worth a flight; if M1's `rate=` readback already shows both
  DACs converging on identical 48000, M4 may be skippable — Peter's call after M1's data is in.

## Next-session start

1. `grep -n "PARAM_" unaos/crates/kernel/src/drivers/hda.rs` to find the exact insertion line for
   `PARAM_SUPPORTED_PCM` (M1) without renumbering existing consts.
2. Land M1 first (read-only, no behavior change, cheapest go-red), rebuild with
   `UNAOS_HDA=1 UNAOS_HDATONE=1 ./arroyo esp-x86` and eyeball `rate=`/`pcmcaps=` on the new amp line
   before touching `fill`/`check` for M2.
3. Only after M1's data is read, decide with Peter whether M2 (fade) or M4 (one-member) flies next;
   write `scripts/specs/x86-hdatone3.spec` once the first real fixture line exists to pin against.

## Draft code (unbuilt)

```rust
// hda.rs, after `pub const PARAM_GPIO_COUNT: u32 = 0x11;` (~line 1297) — M1
pub const PARAM_SUPPORTED_PCM: u32 = 0x0A; // "Supported PCM Size, Rates" [HDA-SPEC §7.3.4.7]
```

```rust
// hda.rs, in tone::fill/check — M2 (sketch only; exact envelope scale TBD next session)
const FADE_FRAMES: usize = SAMPLE_RATE / 50; // 20 ms
fn envelope_q15(f: usize) -> i32 {
    if f < FADE_FRAMES { ((f * 32768) / FADE_FRAMES) as i32 }
    else if f >= FRAMES - FADE_FRAMES { (((FRAMES - f) * 32768) / FADE_FRAMES) as i32 }
    else { 32768 }
}
```

```rust
// hda.rs, after AMPLITUDE's parse_amp — M4
pub const FORCE_ONE_MEMBER: bool = option_env!("UNAOS_HDA_MEMBERS").is_some();
```
