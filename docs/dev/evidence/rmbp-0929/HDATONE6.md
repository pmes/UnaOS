# HDATONE6 — one DAC tones, the other is silent, both screech (boot 18)

## Design
Finding (boot 18, Peter): `tests hda` screech, `hda1` high tone, `hda2` silent. Wire: `[hda] gpio afg=0x01 ... mask=0x0f ... data=0x00->0x0f -> set`
(all four GPIOs high), `eapd ... pincap_eapd=0`, sdfmt/conv match, stream-id tag=1 chan=0 both, pin 0x0a historically `actual=D3`.
Mechanism: Linux patch_cirrus.c (CS420X_MBP101) drives the speaker-amp GPIO (recalled bit 3, 0x08) and HP-amp GPIO (recalled bit 1, 0x02) mutually exclusively
(cs_automute). UNCERTAIN: exact masks recalled from memory; knob `UNAOS_HDA_GPIO` lets the flight sweep. Driving all four is the suspect for the screech.
Code: `drivers/hda.rs` run_tone gpio block (~l.1745), power loop (~l.1860), witness (~l.2230), file-tail test verbs; registration `arch/x86_64/syscall.rs:21539`.

Milestones: M1 Cirrus GPIO = speaker bit high only, HP bit output LOW, `[hda] gpio data=.. speaker_bit= hp_bit=`; M2 pin power poll existed (HDATONE4 loop covers pins
with POWER_CTL) — now prints `[hda] pin power ... settled=` and on non-D0 runs AFG+DAC+pin fallback (`pin power fallback`); M3 `tests hda` = member 0 only
(`members=1 default=solo-primary`), `hdaboth`, `hda220`, `hda880` (member 0 only; runtime pitch override).
Pins: x86-witness HDA rows unchanged (new fields sit between `settled_ms=` and `run_reasserts=`; non-Cirrus keeps whole-set GPIO drive).

## Written
Boot 19 witness: `:: HDA-TONE: ... settled_ms=N gpio=0x08 hp_gpio=0 spk_gpio=1 pin_pwr=[D0,-] run_reasserts=...` for `tests hda`; with `tests hdaboth` `pin_pwr=[D0,D0]`.
