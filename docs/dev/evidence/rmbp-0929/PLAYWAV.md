# PLAYWAV (R75) — a streaming PCM path and a WAV player on the HDA output stream

## Design
**Finding.** `drivers/hda.rs` `run_tone` plays one 192 KB buffer through a 2-entry BDL (`[hda] tone arm … cbl=192000 lvi=1`, fmt 0x0011)
and restores everything; there is no way to play anything else. No `play`/WAV code exists (`grep -rn '"play"' shell.rs` empty).

**Mechanism.** The tone's bring-up (power D0, amps unmuted, GPIO/EAPD, pair walk, converter format + stream-tag bind) is inline in `run_tone`
with local state and cannot be called as a function. So the seam is a gate INSIDE it: `play::gate(base, sd, iss, rings, cad, &paths[..np], a)`
(hda.rs, same-line fold before the SDnCTL reset, `-> bool`; `true` = `run_tone` returns, leaving the codec armed). `hda_play.rs` is
declared at hda.rs's tail as `#[path = "hda_play.rs"] pub mod play;` (a child module: it reaches `Rings`/`Path`/`r8`… privately). Entry points
are `hda::play::{start, feed, finish, stop, service, open_wav, selftest, shell_verb, request_open}`.
* M1 `start(rate,ch,bits)` sets `REQ_RATE`, `TONE_NOW`, runs `probe()` (as `hda_tone_test`); `gate` sets the converter format (`rate_bits`
  BASE/MULT/DIV, 0x4011 for 44.1 k; PCM-rates word and a format READBACK decide, else 48 k and `resampled=1`), builds a ring of 4 x 32 KiB
  BDL entries, IOC on each, programs SDnCBL/LVI/FMT/BDPL/BDPU/tag, and does NOT run. The stream is always 16-bit stereo: `feed` widens 8-bit,
  duplicates mono, resamples nearest-sample. The stream RUNs when the FIFO holds a full ring (or the source ended). `service()` (folded at the
  top of `probe_after_root`, every device-service pass) POLLS `SDnLPIB` (INTCTL is never written — HDASIE is a latch experiment) and refills
  each entry as the engine leaves it; a refill short of 32 KiB while the source is live is an underrun; 4 consecutive silent refills after
  `finish()` stop the stream.
* M2 `play <path.wav>` / `play stop` (shell.rs same-line arm): RIFF/fmt/data walk through the mount table (PCM tag 1, 8/16-bit, 1–2 ch,
  8–48 kHz; refusals print `:: PLAYWAV: path= reason= -> REFUSED ::`), 32 KiB-of-output chunks read on the tick. Quarry: `.WAV` → `Act::Play`
  → `request_open` (latched, the tick starts it).
* M3 `tests playwav`: synthesises a 2 s 440 Hz stereo 48 k WAV (20 ms fades) into `/home/<user>/TEST.WAV`, plays it, unlinks it.

**Witness.** `:: PLAYWAV: path= rate= ch= bits= secs= under= -> PASS|FAIL ::` (PASS = every frame fed, under=0, no FIFOE/DESE, >=1 entry
completed) plus `[play] arm …` / `[play] open …` / `[play] done resampled= …`. **Spec pin.** x86-witness.spec: `REQUIRE :: PLAYWAV: … rate=48000 ch=2
bits=16 secs=2.0 under=0 -> PASS ::`, `FORBID … -> FAIL ::` (the lane runs the full `tests` set; the existing HDA rows are untouched).
No knob (rides `hda-tone`).

## Written
Boot 17 (`tests playwav`, then `play /home/<user>/X.WAV`): `:: PLAYWAV: path=/home/<user>/TEST.WAV rate=48000 ch=2 bits=16 secs=2.0 under=0 -> PASS ::`
preceded by `[play] arm … eff_rate=48000 fmt=0x0011(readback 0x0011) … entries=4x32768 …`. The ear compares it with `tests hda`.
Known limits: after the stream ends the converter stays bound/unmuted and GPIO/EAPD stay driven (no restore, unlike the tone); every `start`
re-walks the codec like `tests hda`; one player at a time.
