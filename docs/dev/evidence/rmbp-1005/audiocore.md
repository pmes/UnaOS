# AUDIOCORE (rmbp-ledger B396) — audio_core's constructors built ~40 KiB values on the stack; FLAC panicked on a mutated frame

## Finding (the wire, then measured — no QEMU)
- Flight 24 card 3 reboot (`f24-boots.log`): `[play] dec spawn path=/system/test-f/TEST.AAC jid=1 stack=65536 cpu=0` →
  `[stack] OVERFLOW task=play-dec … via=df -> task halted`. DECJOBHANG (B386) raised `DEC_STACK` to 160 KiB as the
  stop-gap and named the source-side bound owed to the shared core.
- Measured on `x86_64-unaos.json`, release, `-Z emit-stack-sizes`, `llvm-readobj --stack-sizes` on the crate object
  (the same numbers DECJOBHANG quoted): `aac::Chan` was ~9 KiB of inline arrays (band_type 512 B, sf 2 KiB, coef 4 KiB,
  TNS lpc 2.5 KiB) and `AacDecoder` held two; SILK's `ChannelState` held `exc_q14`/`out_buf`/`cng.exc_buf_q14`
  inline (~4 KiB) and `SilkDecoder` two. Each constructor built the value and moved it outward frame by frame.
- `tests/robust.rs` `flac_mutations_never_panic` / `lossless_mutations_never_panic` panic in a DEBUG build at
  `flac.rs:211:37` — `acc += coef[j] * out[i - 1 - j]`, an i64 overflow: a mutated frame's predictor feeds back
  samples that grow without bound, since nothing checked a residual's width or a predicted sample's range.

## Seam
CHARTER: shared-core — `unaos/libs/media/audio_core` (R79, the midden_core shape; AUDIOCODEC SR30), linked by Ring 0
(`drivers/hda_play.rs`) and Ring 3 (Gneiss `dsp::audio`, `tools/audio-check`). No new file, no knob, no kernel code;
the one kernel touch is the `DEC_STACK` constant, re-derived from the new measurements.

## Milestones
- M1 the AAC per-channel arrays (`Chan.band_type/sf/coef`, `Tns.lpc`) and the SILK channel buffers (`exc_q14`,
  `out_buf`, `cng.exc_buf_q14`) are `Vec`s allocated once at `new`, zeroed in place (`fill`) on reset/per frame.
- M2 FLAC bounded from RFC 9639: a Rice residual wider than 32 bits (§9.2.7) is `Invalid`; each predicted sample
  (fixed and LPC) must lie within the subframe's bit depth (§9.2), checked per sample, which also bounds every
  predictor sum inside i64. A bad frame is an error, never a panic.
- M3 `DEC_STACK` 160 KiB → 64 KiB in `drivers/hda_play.rs` (one line, the derivation in its comment).

## Measured frames (bytes, kernel target, release) — before (a8a5df91) / after
| function | before | after |
|---|---|---|
| `audio_core::mp4::open` | 40424 | 3944 |
| `aac::AdtsStream::new` | 39528 | 3048 |
| `aac::AacSource::new` | 39496 | 3016 |
| `open_arm::adts` | 39352 | 2872 |
| `opus::decoder::OpusDecoder::new` | 25240 | 3064 |
| `audio_core::ogg::open` | 19832 | 6040 |
| `opus::silk::SilkDecoder::new` | 16824 | 1512 |
| `opus::OggOpus::new` | 9208 | 2312 |
| `opus::silk::SilkDecoder::decode` | 8568 | 920 |
| `opus::silk::channel::ChannelState::new` | 4232 | 792 |
| `aac::decoder::AacDecoder::new` | 760 | 1240 |
| `aac::decoder::AacDecoder::decode_ics` | 4920 | 4888 |
| `mp3::Mp3Decoder::new` | 3704 | 3704 |
| `mp3::Mp3Decoder::decode_frame` | 5048 | 5048 |
| `flac::FlacDecoder::new` | 216 | 216 |
| `Decoder::open` | 184 | 184 |

Largest frame left in the crate: `ogg::open` 6040. Every `open` and `new` is under 8 KiB. Worst chains: constructor
`ogg::open` → `OggOpus::new` → `OpusDecoder::new` → `SilkDecoder::new` → `ChannelState::new` ~ 14 KiB; decode
`Mp3Decoder::decode_frame` / `AacDecoder::decode_ics` ~ 6 KiB. With the VFS read chain (~16 KiB, DECJOBHANG's
figure) the task needs ~30 KiB: `DEC_STACK` = 64 KiB, a 2x margin.

## Witness
- Host: `cargo test -p audio_core` debug and `--release`, exit 0 (robust.rs FLAC/lossless mutation cases included).
- Metal, `tests play` / Quarry opens (rides `hda-tone`):
  `[play] dec spawn path=/system/test-f/TEST.M4A jid=<n> stack=65536 cpu=<n> on=worker` …
  `[play] dec exit jid=<n> why=eos stage=… calls=<n> stack high=<n> of 65536` with `<n>` well under 65536 (expected
  ≲ 32 KiB), and no `[stack] OVERFLOW task=play-dec` line for any of FLAC/MP3/OGG/OPUS/M4A/AAC.

## Owed
- The kernel's own share of the play-dec stack (dec_task + the VFS read chain) is DECJOBHANG's estimate, not measured
  here; the flight's `stack high=` is the measurement that confirms or moves `DEC_STACK`.
- The Vorbis/MP3 constructors were already small; no change.
