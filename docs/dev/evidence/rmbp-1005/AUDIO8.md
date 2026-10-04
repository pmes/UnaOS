# AUDIO8 — the speaker amp is a held state, not a per-run bracket (rmbp-ledger B329)

Branch `exec-rmbp-audio8`, cut from 4e48ab03. Answers FLIGHT20 (boot 20: sound works, and it pops).
Seam: `CHARTER: Stria — owed B289` (as AUDIO7: the HDA driver's stream half a Stria fulfiller would call).
New file `unaos/crates/kernel/src/drivers/hda_amp.rs` (a child of `hda`, `hda-tone` only; drivers/ is
outside GATE-CHARTER's directories, the line is carried anyway).

## 1. Finding (f20-boots.log, awk on `[hda]`, `:: HDA`)

- **The pop has two sources, both per run.** (a) Every `tests hda` / `play` re-enters `probe()`, which
  pulses `GCTL.CRST` — a link reset is a codec reset, so the codec's GPIO, pin controls and power states
  fall back to their reset values at the top of every run. (b) `run_tone` then drives the speaker GPIO
  (`[hda] gpio … data=0x00->0x08 … -> set`, `speaker_bit=1`) and, after the stream stops, restores it
  (`[hda] gpio … restored data=0x00`) together with the pin control, EAPD, the out amps and the node power
  states. The class-D amp is switched on and off around every tone and every file: Peter's "analog switch
  slamming".
- **The DAC amp is rampable.** All three DACs report `caps=0x000d041d`: out-amp present (bit 2) with its
  own amp caps (bit 3); the rearm sets gain 115 (`dacamp=0x73/0x73`). The pins have no out amp
  (`out_amp=0`). So M2 is possible on this codec.
- **hdaboth / hda220 / hda2 fail on the FIXTURE, not the driver.** Every one of them scores
  `fields_stable=0` and nothing else (`dac_fmt_match=1 tag_match=1 lpib_moved=1`, `HDA-TONE … PASS`).
  The `diff` they fail on compares against the PREVIOUS tone run whatever its shape:
  `run=2 diff vs run=1: … post=[members]` (hdaboth after the solo hda), `run=3 … post=[members]` (hda220
  after hdaboth), `run=6 … post=[m.path]` (hda2 = DAC 0x03 after hda1 = DAC 0x04), `run=9 … [m.path]`
  (hda after hda2). The second member's wire is correct: `m1 dac=0x03 conv=0x0011 sc=0x10
  dacamp=0x73/0x73 dpwr=D0 pin=0x0a pinctl=0x40`, bound to tag 1 chan 0 (both members carry the stereo
  pair, the Linux extra-speaker shape), LPIB walked at 192000 B/s. "Silent or odd" by EAR is not on the
  wire; it stays a bench question.

## 2. Milestones

- **M1 AMP STATE** (`hda_amp.rs`). The speaker GPIO is raised by the first run that plays (tone or
  stream) and HELD: while it is held `reset()` takes the warm path (no CRST — the controller and codec are
  already up; the stream descriptor is still reset per run by `stream::rearm`'s SRST pulse), the GPIO drive
  is skipped, and the end-of-run codec restore (stream/channel, format, EAPD, pin control, out amps, power,
  GPIO) is skipped. Streams `acquire`/`release` (bits: tone, play); after the last release an idle hold-off
  (default 5000 ms, `AMP_HOLDOFF_MS`; the Principia pref `system`/`audio.amp_holdoff_ms`, 0..=600000,
  read with `prefs::peek_int`) runs from the device-service tick (`play::service`), then the GPIO goes back
  to the firmware's data/dir/enable once. Shutdown/reboot (`powerdown::stop`) drops a held amp and restores
  the member pins' firmware pin control + EAPD (there is no sleep path in this tree: owed). One line per
  transition: `[hda] amp=up why=first-play …`, `[hda] amp=down why=idle …` (`why=shutdown` at power-off).
- **M2 RAMP.** The DAC out amp (the converter's own output amplifier) is stepped, not switched: before RUN
  it is set to gain 0 muted (the converter is idle, so this is inaudible), after RUN it ramps to the
  rearm's gain in 10 steps over ~10 ms, and before STOP it ramps back to 0 and mutes. Tone: inside
  `run_tone` on its rings. Play: prepared in `gate`, ramped in `ring_pump`/`hw_stop` through the amp
  module's own CORB/RIRB pair (`svc_rings`: allocated once, re-pointed on use, stopped after).
- **M3 FIXTURE.** `stream::rearm` keeps the last run PER SHAPE (who, SDxFMT, member DAC/pin list) and diffs
  only like against like; a new shape reads `diff vs run=- … (first <who> run of shape …)` and is stable.
  The driver is unchanged for the second DAC.
- **M4 WITNESS.** Every tone run ends with
  `:: AUDIO8: runs=<tone runs since the up> plays=<n> amp_up=<ups in the window> amp_down=<downs in the window> pops_bracketed=<GPIO writes/restores inside a held window> -> PASS|PENDING ::`
  (PASS needs runs ≥ 3 with one up, no down, no bracket; PENDING before the third run; FAIL on a bracket).

## 3. What a metal boot prints

```
[hda] amp=up why=first-play gpio=0x00->0x08 holdoff_ms=5000 pins=1
[hda] ramp dac=0x04 out_amp=1 steps=127 target=115 step_ms=1 n=10 (once per DAC)
:: HDA: runs=1 fields_stable=1 … -> PASS ::
:: AUDIO8: runs=1 plays=0 amp_up=1 amp_down=0 pops_bracketed=0 -> PENDING :: …
:: HDA: runs=2 … -> PASS ::
:: AUDIO8: runs=2 … -> PENDING :: …
:: HDA: runs=3 … -> PASS ::
:: AUDIO8: runs=3 plays=0 amp_up=1 amp_down=0 pops_bracketed=0 -> PASS :: …
[hda] amp=down why=idle held_ms=… gpio=0x08->0x00 readback=0x00      (5 s after the last run)
```
No `[hda] gpio … restored` and no `[hda] gpio … -> set` between runs. `play <wav>` then `tests hda` inside
5 s: no `amp=` line between them, the AUDIO8 line reads `plays=1`.

## 4. Owed

- Sleep: no S3/suspend path exists in this tree; the shutdown hook is the only drop besides idle.
- The VOLKEYS rings (`vol::RINGS`) are cached once and go stale after any later `Rings::init` (pre-existing,
  not changed here).
- Stria owns `play` (B289/BANDY3). Ear: is member 1 (DAC 0x03 -> pin 0x0a) audible on `tests hda2`.
