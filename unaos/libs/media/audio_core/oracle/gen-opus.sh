#!/bin/sh
# AUDIOCODEC (SR30): how tests/data/opus/ was made — the normative reference (libopus 1.5.2, RFC 6716 §1)
# encodes a deterministic test signal with its own conformance modes (the ones the RFC 6716/8251 test
# vectors were made with), and the FIXED-POINT reference decoder produces the expected PCM, recorded as an
# MD5 in expected*.txt (the .dec files are 0.5–1.2 MB each and are not committed).
#   git clone https://github.com/xiph/opus && cd opus && git checkout v1.5.2
#   make -f Makefile.unix opus_demo                         # float build (encoder; either works)
#   cp -r . ../opus-fixed && make -C ../opus-fixed -f Makefile.unix FIXED_POINT=1 opus_demo
# opus-sig.py (next to this script) writes the
# 48 kHz s16 stereo signal: formant-filtered pulse train, chord, noise bursts, transients, chirp, quiet tail.
set -e
E=${OPUS_FIXED:-../opus-fixed}/opus_demo
python3 opus-sig.py 6.0 sig6.raw; python3 opus-sig.py 3.0 sig3.raw   # + mono copies sig6m.raw / sig3m.raw (left channel)
$E -e voip 48000 2 12000 -silk8k_test sig6.raw t01.bit
$E -e voip 48000 2 16000 -silk12k_test sig6.raw t02.bit
$E -e voip 48000 2 20000 -silk16k_test sig6.raw t03.bit
$E -e voip 48000 2 32000 -hybrid24k_test sig6.raw t04.bit
$E -e audio 48000 2 48000 -hybrid48k_test sig6.raw t05.bit
$E -e audio 48000 2 64000 -celt_test sig6.raw t06.bit
$E -e audio 48000 2 160000 -celt_hq_test sig6.raw t07.bit
$E -e audio 48000 2 96000 -random_framesize sig6.raw t08.bit
$E -e voip 48000 1 16000 -sweep 2000 -sweep_max 64000 sig3m.raw t09.bit
$E -e audio 48000 2 32000 -sweep 8000 -sweep_max 96000 -random_framesize sig3.raw t10.bit
$E -e restricted-lowdelay 48000 1 32000 -framesize 2.5 sig6m.raw t11.bit
$E -e voip 48000 1 20000 -inbandfec -loss 10 -dtx sig6m.raw t12.bit
for i in 01 02 03 04 05 06 07 08 09 10 11 12; do $E -d 48000 2 t$i.bit t$i.dec; done
for i in 03 05 06 09 10 12; do $E -d 48000 2 -lossfile loss_a.txt t$i.bit t${i}_loss.dec; done
