#!/bin/sh
# AUDIOCODEC (SR30): how tests/data/aac/ was made. Encoder: fdk-aac 2.0.3 (github.com/mstorsjo/fdk-aac,
# its aac-enc example, ADTS out). Reference decoder: faad2 2.11.4 (github.com/knik0/faad2, float build), a
# decoder written independently of ours and of fdk's; its 16-bit output is stored as FLAC (xiph flac 1.4,
# -8) next to each stream as <name>.ref.flac. tests/aac_kat.rs decodes the FLAC with our own (bit-exact) FLAC
# decoder and holds our AAC decode to it. adts2mp4.py (next to this script) wraps an ADTS stream into an
# MP4 with an edit list, independently of src/mp4.rs.
# Build: cmake -S fdk-aac -B b -DBUILD_PROGRAMS=ON && cmake --build b ; same for faad2 and flac. faad2's
# frontend upsamples every stream at <= 24 kHz by 2 on the assumption of implicit SBR; these are plain LC
# streams, so un-comment `config->dontUpSampleImplicitSBR = 1;` in faad2/frontend/main.c (both places).
# Since the decodes go out as raw PCM, faad keeps the native channel order for channelConfiguration 3
# (C L R; tests/aac_kat.rs remaps to WAVE); 5.1 comes out in WAVE order.
set -e
: "${ENC:=aac-enc}" "${FAAD:=faad}" "${FLAC:=flac}"
gen() { # seconds rate channels bitrate name [vbr]
  python3 vorbis-sig.py $1 $2 $3 s.raw
  python3 -c "import struct,sys;d=open('s.raw','rb').read();r,c=$2,$3;open('s.wav','wb').write(b'RIFF'+struct.pack('<I',36+len(d))+b'WAVEfmt '+struct.pack('<IHHIIHH',16,1,c,r,r*c*2,c*2,16)+b'data'+struct.pack('<I',len(d))+d)"
  if [ -n "$6" ]; then $ENC -v $6 s.wav $5.aac; else $ENC -r $4 s.wav $5.aac; fi
  $FAAD -f 2 -o r.raw $5.aac >/dev/null 2>&1
  $FLAC -s -f -8 --force-raw-format --endian=little --sign=signed --channels=$3 --bps=16 --sample-rate=$2 -o $5.ref.flac r.raw
}
gen 0.75 44100 2 128000 a01
gen 0.75 44100 1 24000 a02
gen 0.75 48000 2 40000 a03
gen 0.75 22050 2 64000 a04
gen 1 8000 1 12000 a05
gen 0.4 96000 2 192000 a06
gen 0.5 32000 6 192000 a07
gen 0.75 32000 2 0 a08 3
gen 0.75 16000 1 20000 a09
gen 0.5 44100 3 160000 a10
gen 0.75 11025 1 16000 a11
gen 0.5 64000 2 96000 a12
# MP4 wrappings (container KATs; the reference is the ADTS stream's, offset/length per expected.txt)
python3 adts2mp4.py a01.aac m01.m4a --skip 2048 --dur 30000
python3 adts2mp4.py a07.aac m07.m4a --frag 5
python3 adts2mp4.py a10.aac m10.m4a --skip 1024 --dur 20000 --smpb --moov-last
python3 adts2mp4.py a04.aac m04.m4a
