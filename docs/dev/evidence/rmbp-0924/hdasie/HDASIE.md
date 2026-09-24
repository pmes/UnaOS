# HDASIE — the INTCTL.SIE latch experiment, QEMU mechanics (rmbp-ledger B207, 2026-09-24)

Cloud session, no bench, no KVM (QEMU 8.2.2 TCG, `UNAOS_QEMU_MACHINE=pc-q35-8.2`, OVMF 4M aliased).
QEMU's `hda-duplex` latches BCIS with or without SIE, so this proves the knob's MECHANICS only; the
bench answers the question (hda.md §6b). Both runs' serial logs were read with `awk 'index($0,"[hda]")'`.

## Run 1 — knob ON

`UNAOS_QEMU_MACHINE=pc-q35-8.2 UNAOS_WC=1 UNAOS_HDA=1 UNAOS_HDATONE=1 UNAOS_HDASIE=1 UNAOS_QEMU_FULL=1 ./arroyo test 120`
→ **rc=1**, the ONE fault line being the container's `:: SOCK-3: ring-3 tcp round-trip FAIL …` (proven
environmental on the untouched tip earlier this session, LOGIN14.md); no other `-> FAIL` on the wire.

```
[hda] rings corb=0x17bea00 rirb=0x334d000 entries=256/256 corbctl=0x02 rirbctl=0x03 rintcnt=1 intctl=0x00000000(untouched)
[hda] intctl sie desc=4 bit=0x00000010 before=0x00000000 want=0x00000010 after=0x00000010 set=1 gie=0 cie=0
[hda] intctl restore desc=4 armed=0x00000010 saved=0x00000000 after=0x00000000 restored=1 bcis_with_sie=2
:: HDA-SIE: desc=4 sie_set=1 restored=1 bcis=2 -> PASS ::
[hda] tone stream=0 lpib=0 -> 44176 (max 191824) bcis=2 fifo_ready=1 run_ms=1200 sts=0x20 fifoe=0 dese=0 cbl=192000 tag=1 tag_bound=0x10 tag_ok=1 wraps=1 consumed=236176 rate_bps=196813 expect_bps=192000 members=1 ctl_running=0x00100006
:: HDA-TONE: lpib_advanced=1 walked=1 wraps=1 bcis=2 tag_ok=1 fifo_ready=1 run_ms=1200 members=1 -> PASS ::
[hda] audit stage=tone wrote-cfg=0 wrote-ctrl=102 wrote-stream=24 verbs-get=32 verbs-set=9 wrote-intctl=2(sie) wrote-wallclk=0(audited) wrote-dplbase=0(audited)
```

Read: `desc=4` is `iss` (the first output descriptor of a 4-in/4-out `hda-duplex` controller), so the
bit is `0x10`; `before=0` (the audited never-written register), `set=1 gie=0 cie=0` (only that bit),
`restored=1`, the tone verdict unchanged (`-> PASS`, `bcis=2`), and the audit line now COUNTS:
`wrote-intctl=2(sie)` in place of `0(audited)`. `[hda] rings … intctl=0x00000000(untouched)` still
prints before the tone and is still true there.

## Run 2 — go-red by mutation

`let restored = after == saved;` → `after != saved` in `sie_restore`, same line, rebuilt, same command:

```
:: HDA-SIE: desc=4 sie_set=1 restored=0 bcis=2 -> FAIL ::
✖ serial.log:1973: :: HDA-SIE: desc=4 sie_set=1 restored=0 bcis=2 -> FAIL ::
```
→ rc=1 with the HDA-SIE line named in the verb's fault text (DEFAULT_FORBIDS `-> FAIL`). Source
restored afterwards (`grep -c 'GO-RED MUTATION' drivers/hda.rs` = 0 before the commit).

## Not run here

`./arroyo knoboff hda-sie`: the module is not lexed knob-off (hda.md §6 Byte identity), so the
argument is the same as `hda-tone`'s; the measurement is owed on the bench with the rest of the
fold gate. The bench flight line is in hda.md §6b and the rmbp queue's HDASIE FLIGHT row.
