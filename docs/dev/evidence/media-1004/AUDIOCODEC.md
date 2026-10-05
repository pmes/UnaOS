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

## M2 — Opus (RFC 6716, RFC 8251; Ogg Opus RFC 7845)

**The specification is the reference code.** RFC 6716 §1 makes the reference implementation normative
(the prose is informative), so `src/opus/` is a Rust rendering of libopus 1.5.2's FIXED-POINT decoder —
the build whose output is integer and deterministic, which is what the kernel wants and what lets the KAT
be bit-exact. Every fixed-point macro is reproduced with its casts (`celt/fixed.rs`, `silk/macros.rs`:
the 16-bit truncation inside `MULT16_16`, `ADD16`'s wrap, the 64-bit-host `OPUS_FAST_INT64` forms); the
tables (CELT static mode, SILK codebooks) are extracted verbatim from the reference sources by a script.
No dependency.

* `range.rs` — §4.1 range decoder, raw end bits, `ec_tell`/`ec_tell_frac`, final range.
* `silk/` — §4.2: LBRR and VAD flags, stereo predictor and mid-only flag, frame-type/gain/NLSF
  (two-stage VQ, residual dequant, stabilisation, NLSF→LPC with the 16-bit fit and the inverse-prediction-
  gain bandwidth expansion), pitch lags and contours, LTP codebooks and scaling, shell-coded pulses with
  LSB extension and signs, the excitation/LTP/LPC synthesis (`decode_core`), mid/side→L/R with predictor
  interpolation, the SILK PLC (pitch-repeating, energy-matched glue) and comfort-noise generation, and the
  output resampler (2x all-pass + 12-phase FIR, 8/12/16 kHz → 48 kHz).
* `celt/` — §4.3: silence/post-filter/transient/intra flags, Laplace coarse energy with prediction, fine
  and final energy, TF changes, spreading, dynamic allocation boosts, trim, the full bit allocator with band
  skipping and intensity/dual-stereo signalling, PVQ codeword decoding (`cwrs`), the spreading rotation,
  recursive band splitting with the theta angle, stereo merge/inversion, Haar/Hadamard time-frequency
  reshaping, folding with the hybrid special case, anti-collapse, denormalisation, the KISS FFT (radix
  2/3/4/5) and inverse MDCT with TDAC, the pitch pre/post comb filter, de-emphasis (with SILK accumulation),
  and both PLCs (pitch-based with LPC extrapolation; noise-based).
* `decoder.rs` — §3/§4.5: TOC, frame packing codes 0–3 with padding, SILK/hybrid/CELT dispatch, the
  5 ms redundant CELT frames on mode switches, smooth cross-fades, CELT→SILK silence fade, transition PLC,
  in-band FEC (`decode_ext(.., fec)`), decode gain, final range.
* `mod.rs` — RFC 7845: `OpusHead`, `OpusTags`, pre-skip, output gain, the granule of sample 0 taken from
  the first page (streams may start at a granule > 0 — Chromium's `bear-opus.ogg` does) and end trimming
  from the last page's granule. Channel mapping family 0 only (mono/stereo); multistream is owed.

**KATs (bit-exact).** The RFC 8251 `opus_testvectors` are only hosted on opus-codec.org, which this
container cannot reach, so the vectors were made the way those were: the reference encoder's own
conformance modes (`opus_demo -silk8k_test … -celt_hq_test`, bitrate sweeps, random frame sizes,
restricted-lowdelay 2.5 ms, FEC+DTX), recipe in `oracle/gen-opus.sh`, the twelve `.bit` streams (530 KB)
committed in `tests/data/opus/`, the expected PCM of the libopus 1.5.2 fixed-point decoder recorded by MD5.
* `tests/opus_kat.rs::opus_vectors_bit_exact` — **12/12 streams, 7,330 packets: every packet's final range
  equals the encoder's (the RFC 6716 §6 conformance check), and all PCM is bit-identical** (MD5) to
  `opus_demo -d 48000 2` (SILK NB/MB/WB 10–60 ms mono/stereo, hybrid SWB/FB, CELT 2.5–20 ms at 64 and
  160 kb/s, code-3 multi-frame packets up to 120 ms, live mode switching with redundancy and transitions).
* `opus_packet_loss_bit_exact` — six of the streams with a fixed 12 % loss pattern (`loss_a.txt`) decoded
  the way `opus_demo -lossfile` does: **6/6 bit-identical**, 417 concealed frames (SILK PLC, CELT pitch and
  noise PLC, transition PLC) and 19 frames rebuilt from in-band FEC.
* `tests/robust.rs::opus_mutations_never_panic` — 7,330 packets bit-flipped / truncated / garbage-filled
  through live decoders: never a panic.

**Chromium oracle (SNR).** `tests/lossy_oracle.rs` muxes the twelve streams into Ogg Opus (pre-skip 312,
end trim 101) and fetches Chromium's `bear-opus.ogg`, `sfx-opus.ogg`, `opus-trimming-test.ogg`: **15/15
frame counts equal, SNR 55.5–93.4 dB**. Chromium runs libopus in floating point; ours is the fixed-point
reference, and the gap is the reference's own: `opus_demo` fixed vs `opus_demo` float on t10 measures
55.51 dB, exactly what ours shows against Chromium (t06: 67.57 vs 67.6). The Opus floor is therefore 50 dB;
the bit-exact KAT is the proof.

Decode speed (release, host, loaded 4-core box): 160 kb/s stereo CELT 47x realtime, SILK WB 166x realtime.

## M3 — Vorbis (Xiph Vorbis I specification)

`src/vorbis/` is written from the specification (2020-07-04 revision), floating point:
* §4.2 headers: identification (blocksizes 64–8192), comment (skipped), setup — codebooks (§3: ordered,
  sparse and dense length lists, the codeword assignment, VQ lookup types 1 and 2 with `float32_unpack`
  and `lookup1_values`, single-entry books), time-domain placeholders, floors 0 and 1, residues 0/1/2,
  mappings (submaps, coupling steps, mux), modes, framing bit. Hostile values are bounded (VQ tables,
  floor1 points, book indices) before anything is allocated.
* §4.3 audio packets: mode and window flags, floor decode per channel with the end-of-packet rule (an EOP
  inside a floor zeroes the channel; inside a residue it ends the residue), nonzero propagation through
  coupling, residue decode (type 2 interleaved), inverse polar coupling, floor 1 curve synthesis (§7.2.4:
  amplitude prediction, `render_line`, `floor1_inverse_dB_table`), floor 0 (§6: LSP curve over the bark
  map), the power-sine windows with long/short transitions, the inverse MDCT (an N/2 DCT-IV through an
  N/4-point complex FFT, unit-tested against the definition), overlap-add.
* §A Ogg mapping: the first page's granule (samples to discard at the start, except when the first page is
  also the last — then the end is cut, as the spec and libvorbis say), end trimming to the last granule.
* Channel order: the API hands out WAVE/SMPTE order for 3–8 channels (the §4.3.9 Vorbis order L,C,R,…
  remapped), the order every other format here and FFmpeg/Chromium use.

**Vectors.** Ten streams made with libvorbis 1.3.7-git (`oracle/gen-vorbis.sh`, 334 KB in
`tests/data/vorbis/`): 8 k–96 kHz, mono/stereo/3/6 channels, VBR quality −0.1…1.0 and a managed 64 kb/s
stream; plus Chromium's `sfx.ogg` (a single-page stream: first page = last page) and `9ch.ogg`.

**Oracle.** Against libvorbis's own float decoder (`vdec`, vorbisfile `ov_read_float`): **12/12 streams,
identical frame counts, SNR 135.3–136.3 dB, max |d| ≤ 3.6e-7** (float rounding; the MDCTs differ in
algorithm). Against Chromium (`tests/lossy_oracle.rs`): **11/11 at SNR 136.2–137.7 dB**; Chromium does not
apply the Vorbis end trim (it returns every decoded sample — 15,936 for `sfx.ogg` where libvorbis and the
spec give 15,435), so for Vorbis the frame rule is "equal to libvorbis, not longer than Chromium";
Chromium refuses `9ch.ogg`, which is proven against libvorbis alone. Robustness: 600 mutated setup headers
and 1,581 mutated audio packets, no panic.

Ceiling: floor 0 is implemented from §6 but no available encoder produces it (libvorbis has written floor 1
since 2001), so it is untested. Start trimming follows the spec (all the extra samples are discarded);
libvorbis only discards what lies in the last packet of the first page — a deliberate divergence on cut
streams. Speed: 37x (6 ch) – 99x (stereo) realtime on this host; not yet optimised.
