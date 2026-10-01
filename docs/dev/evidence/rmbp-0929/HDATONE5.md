# HDATONE5 — the tone still screeches after amplitude, buffer, rate, D0 and RUN were excluded

## Finding (boot 17, f17-boot1.log; FLIGHT17.md §2)
`:: HDA-PCM: amp=4096 … sine_ok=1 le_ok=1 … -> PASS ::`; `[hda] power settle … actual=D0`; `[hda] amp member=0 dac=0x04 … fmt_conv=0x0011 fmt_match=1 rate=48000`; member 1 dac 0x03 / pin 0x0a, same; HDA-TONE PASS. Peter: "the tone is still screeching". Left open: (b) two DACs on one tag, (c) SDxFMT readback, (d) container, (e) stream/channel nibble, (f) EAPD/amp, (g) Cirrus specifics.

## Mechanism (drivers/hda.rs)
- The GPIO whole-set drive and the pincap-gated EAPD already exist (`run_tone`, gpio block, `PINCAP_EAPD` gate) — CS4206 reports pincap EAPD clear, so EAPD was never written. `UNAOS_HDA_MEMBERS=1` exists (HDATONE3 M4, `tone::FORCE_ONE_MEMBER`, a build knob).
- New at the file tail (HDATONE5 banner): `disc_read` (M1), `solo_trim` + `hda_tone_test_m0/_m1` (M2), `eapd_force`/`eapd_restore` (M3). Call sites are same-line folds in `run_tone`: after `saved_lvi` (eapd_force), after `fmt_back` (disc_read), after `let sd` (solo_trim), before the GPIO restore (eapd_restore).
- Cirrus (vendor 0x1013): Linux patch_cirrus.c switches the speaker amp by GPIO (`gpio_eapd_speaker`/`gpio_eapd_hp` per Apple fixup; automute flips it). Exact bits per MacBook model are not verified here; the existing whole-GPIO drive plus its `stuck=` field covers them.

## Milestones
M1 discriminators; M2 `tests hda1` (member 0) / `tests hda2` (member 1), `[hda] tone member-solo=`; M3 EAPD forced with before/after, Cirrus note.

## Written
Boot 18 `tests hda`: `[hda] sdfmt=0x0011 conv=0x0011 match=1 member=N`, `[hda] stream-id member= tag= chan=`, `[hda] afg=0x01 power= gpio data= enable= dir= vendor=1013:4206`, `[hda] eapd member= pin= pincap_eapd= … before= after= stuck=`, and `:: HDA-TONE: … stall_reasserts=N sdfmt=0x0011 chan=[0, 0] eapd=[..] vendor=1013:4206 ::`. Spec pins (x86-witness.spec): sdfmt/chan/vendor shape, sdfmt match=1, stream-id tag=1 chan=0; EAPD not pinned.
