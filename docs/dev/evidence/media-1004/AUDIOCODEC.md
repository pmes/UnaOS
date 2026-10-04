# AUDIOCODEC — the audio decoders UnaOS owns (ledger SR30)

Branch `exec-media-audio`, cut from `d6376f33` (a descendant of `0686cc1b`). Crate
`unaos/libs/media/audio_core` (`no_std` + `alloc`, `#![forbid(unsafe_code)]`, **zero dependencies, zero
dev-dependencies**), a root-workspace member; the kernel links it by path (M5). Host faces:
`libs/gneiss_pal::dsp::audio` (re-export) and `tools/audio-check`.

## Finding

Peter (2026-10-04): "we will need to support more than wav files!" Before this arc the kernel's `play`
(`drivers/hda_play.rs`, PLAYWAV/R75) parsed RIFF/WAVE 8/16-bit PCM only, and the host's `libs/resonance` is a
synthesis engine with no file decoding. Nothing in the tree decoded FLAC, Ogg, Opus, Vorbis, MP3 or AAC.

## The API (one for every format)

```rust
pub fn sniff(head: &[u8]) -> Format;              // Wav | Aiff | Flac | Ogg | Mp3 | Adts | Mp4 | Unknown
pub trait AudioDecoder {
    fn open(src: Box<dyn Read>) -> Result<Self>;  // forward-only no_std reader; Decoder::open_bytes(Vec<u8>)
    fn info(&self) -> Info;                       // rate, channels, bits, frames: Option<u64>, format, codec, float
    fn next(&mut self, out: &mut [f32]) -> Result<usize>;      // interleaved, returns frames (0 = end)
    fn next_i32(&mut self, out: &mut [i32]) -> Result<usize>;  // interleaved, LEFT-JUSTIFIED (s << (32-bits))
}
pub fn decode_all(bytes: &[u8]) -> Result<(Info, Vec<f32>)>;   // + decode_all_i32
```

`Decoder` is the concrete type (`open` sniffs and picks the codec); every codec is a `Source`
(`info` + `block(&mut Pcm)`), so a demuxer that already has packets (PLAYBACK's MP4/WebM) can wrap a codec
source directly with `Decoder::from_source`.

**Sample rule (documented, no dither anywhere).** Integer source of depth `b` → f32 `s / 2^(b-1)` (exact for
`b ≤ 24`); → i32 left-justified, so `>> 16` is 16-bit for any source. Float → i32 `round(clamp(x)·2^31)`.

**Chromium's rule differs, and the oracle applies it.** Chromium does not divide symmetrically: FFmpeg hands
it u8 (WAV ≤ 8-bit), s16 (any other source ≤ 16 bits) or s32, and `media/base/audio_sample_types.h` scales
positive values by `1/(2^(n-1)-1)` and negative values by `1/2^(n-1)` (multiplication by the f32 reciprocal).
For s32 the f32 reciprocal of `2^31-1` *is* `2^-31`, which is why every ≥ 20-bit and float file matched
untouched on the first run while 8/12/16-bit files differed by ≤ 1 LSB of the positive half. The oracle test
maps our integers through Chromium's rule (`tests/oracle.rs::chromium_rule`) and then demands bit-equality.
Our API keeps the symmetric rule (it is the one every spec and every other decoder uses).

## Oracle method

`oracle/chromium-oracle.cjs` (Playwright + the pre-installed Chromium 1194) runs `decodeAudioData` in an
`OfflineAudioContext` created **at the file's own rate** (so no resampler sits between codec and dump), one
browser session for a whole job list, and dumps planar f32. `tests/common/mod.rs::chromium` drives it from
`cargo test` (cached under `tests/vectors/oracle/`; no node/Chromium → the vector is skipped, never failed).
Lossless: bit-exact after the rule above. Lossy: SNR and spectral-peak agreement (M2–M4).

This Chromium build has **no AAC, no ALAC and no AIFF** (`decodeAudioData` refuses them;
`AudioDecoder.isConfigSupported('mp4a.40.2')` is false). AIFF is therefore proven against the generator's
own samples (exact by construction), and AAC's oracle is stated in M4.

## M1 — WAV, AIFF, FLAC, Ogg

Spec sections:
* **WAV**: RIFF chunk walk (odd-length pad, unknown chunks skipped), RF64/BW64 `ds64`, `fmt ` tags 1 (PCM
  1–32 bits in 1–4-byte containers; 8-bit unsigned), 3 (float 32/64), 6/7 (G.711 A-law/µ-law, ITU-T G.711
  tables), `0xFFFE` WAVE_FORMAT_EXTENSIBLE (SubFormat GUID → tag; `wValidBitsPerSample` < container honoured,
  samples left-justified), streamed WAV with a zero data size (read to EOF).
* **AIFF 1.3 / AIFF-C**: `COMM` (80-bit extended rate decoded exactly), `SSND` offset, 1–32-bit signed
  big-endian left-justified, `NONE`/`twos`/`sowt`/`fl32`/`fl64`.
* **FLAC, RFC 9639**: §8.1 metadata block walk, §8.2 STREAMINFO, §9.1 frame header (both blocking strategies,
  every block-size/rate/sample-size code, UTF-8-coded number, CRC-8), §9.2 subframes CONSTANT, VERBATIM, FIXED
  0–4, LPC 1–32 (precision, shift, negative shift refused), wasted bits, §9.2.7 residual coding methods 0/1,
  partition orders 0–15, escape partitions (including 0-bit escapes), §4.2 the four channel assignments (the
  side channel carries bps+1; 32-bit audio's 33-bit side channel and every predictor run in i64), §9.3 frame
  CRC-16, §8.2 **MD5 of the decoded stream checked at end of stream** — a mismatch is an error. A
  **headerless** stream (frames only, Gecko's `flac-noheader-s16.flac`) is sniffed by a CRC-8-valid frame
  header and decoded from the header's own parameters. FLAC-in-Ogg (the Xiph mapping) shares the frame decoder.
* **Ogg, RFC 3533**: page capture with resynchronisation, CRC-32 (poly 0x04C11DB7, init 0) checked — a bad page
  is dropped, the packet spanning it discarded, never spliced; lacing reassembly across pages; granule on the
  last packet completed per page; the first logical stream is followed, other serials skipped.

KATs (`cargo test -p audio_core --release`):
* **FLAC decoder testbench, subset (ietf-wg-cellar/flac-test-files): 62/62 files, every one MD5-exact** against
  its STREAMINFO signature (54 by default + the 8 files > 5 MB with `AUDIO_CORE_BIG=1`; files 50 and 55 — a
  15.8 MB JPEG PICTURE and its combination — are not listed). Covers block sizes 16…16384, variable block size
  (current and old Flake signalling), partition order 8 with escapes, qlp precision 2…15, 32nd-order LPC,
  wasted bits, 8/12/16/20/24-bit, 22.05…384 kHz incl. 35467 and 134560 Hz, 1–8 channels, the 32-bit
  predictor-overflow traps, escape code zero, every metadata extreme.
* MD5 (RFC 1321 A.5 suite), CRC-8/CRC-16/CRC-32 (catalogue check values), G.711 table end points, 80-bit
  extended rates, MSB/LSB bit readers.
* Ogg container: packets of 0/1/255/256/70000/510/3 bytes under 1, 2, 7 and 255 lacing values per page, read back
  exactly; junk before the first page skipped; a corrupted page dropped by CRC with no spliced packet returned.
* Hostile input (`tests/robust.rs`): 942 truncated or bit-flipped decodes of every lossless and FLAC vector —
  Ok or a named Err, never a panic. (`ByteStream` grows in ≤ 1 MiB steps so a lying length field cannot make
  the kernel allocate what the source never delivers.)

**Chromium oracle: 43/43 bit-exact** — 9 generated WAVs (u8 8 kHz, s16, s24, s32 96 kHz, f32, f64, EXTENSIBLE
24-in-32, EXTENSIBLE 6-channel, EXTENSIBLE 20-in-24), 22 fetched files (Chromium's `sfx_*` u8/s16/s24/f32 and
4-channel WAVs, `sfx.flac`, `bear.flac`, `bear-flac.ogg`; Gecko's `wavedata_*` u8/s16/s24/float/A-law/µ-law,
`r11025_*`, the extra-metadata WAV, `small-shot.flac`, the 88.2 kHz 24-bit `flac-s24.flac`, the headerless
`flac-noheader-s16.flac`, `sin-441-1s-44100.flac`) and 14 testbench files (all predictor kinds, 8/12/20/24-bit,
3 and 8 channels). Frame counts equal on every file. AIFF: 7 variants (s8, s12, s16, s24 AIFC, s32, `sowt`,
`fl32`) exact against the generator.

`tools/audio-check <file> [--out pcm.wav]` prints the stream facts, peak/RMS and decode speed and writes WAV.
Speed today (release, host): the 8-channel 192 kHz 24-bit testbench file 44 decodes at 6x realtime (23 M
samples in 2.5 s, MD5 included); 44.1 kHz stereo is several hundred times realtime.
