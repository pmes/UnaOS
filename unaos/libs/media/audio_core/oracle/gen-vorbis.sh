#!/bin/sh
# AUDIOCODEC (SR30): how tests/data/vorbis/ was made. libvorbis (github.com/xiph/vorbis) + libogg
# (github.com/xiph/ogg) built from source; venc.c / vdec.c (next to this script) are 40-line programs over
# vorbisenc / vorbisfile: venc encodes raw s16 at a given channel count, rate and VBR quality (or managed
# bitrate), vdec writes libvorbis's own float decode (interleaved f32) — the reference our decoder is
# measured against (tests/lossy_oracle.rs::vs_reference_library with AUDIO_CORE_LIBREF=<dir>).
#   gcc -O2 -I. -Iogg/include -Ivorbis/include -Ivorbis/lib vorbis/lib/{mdct,smallft,block,envelope,window,
#       lsp,lpc,analysis,synthesis,psy,info,floor1,floor0,res0,mapping0,registry,codebook,sharedbook,lookup,
#       bitrate,vorbisenc,vorbisfile}.c ogg/src/{bitwise,framing}.c venc.c -o venc -lm   (same for vdec)
set -e
gen() { python3 vorbis-sig.py $1 $2 $3 s.raw && ./venc s.raw $3 $2 $4 $5.ogg $6; ./vdec $5.ogg $5.f32; }
gen 3 44100 2 0.1 v01
gen 3 44100 1 -0.1 v02
gen 2 48000 2 1.0 v03
gen 3 22050 2 0.4 v04
gen 4 8000 1 0.0 v05
gen 1.5 96000 2 0.5 v06
gen 2 48000 6 0.3 v07
gen 3 32000 2 0 v08 64
gen 3 11025 1 0.6 v09
gen 2 44100 3 0.2 v10
