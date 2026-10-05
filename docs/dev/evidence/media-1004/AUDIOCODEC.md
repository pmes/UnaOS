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
// Read and Source are `Send`: a decoder can move to an audio thread (host) or live in the kernel's player state
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

**Per-packet entry points** (for a demuxer that already holds packets — PLAYBACK's `dsp::audio_track`
registry, which today reports compressed tracks `Unsupported` until such a seam exists):
`opus::OpusDecoder::new(ch)` + `decode(Some(pkt), &mut [i16], frame_size)`; `vorbis::Setup::parse(ident,
setup)` + `VorbisDecoder::decode(pkt, &mut planes)`; `aac::Asc::parse(esds_dsi)` → `AacDecoder::new(sf_index,
layout)` + `decode_block(&mut BitReader::new(au), 0)` (1024 frames in `out`); `mp3::Header::parse` +
`Mp3Decoder::decode_frame`. Wiring them into `audio_decoder_for` is the PLAYBACK×AUDIOCODEC fold's job; note
both branches create `libs/gneiss_pal/src/dsp/mod.rs` — the fold takes the union (`audio`, `audio_track`,
`avsync`, `video`).

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

## M4a — MP3 (ISO/IEC 11172-3 and 13818-3 Layer III)

`src/mp3/` — a float Layer III decoder written against the ISO text, laid out like FFmpeg's
`mpegaudiodec_template.c` (Huffman lengths/symbols and the synthesis window `ff_mpa_enwindow` are the
only tables taken from there, as data; codes are assigned from the lengths). MPEG-1, MPEG-2 LSF and
MPEG-2.5; free format; CRC skipped; the bit reservoir (main_data_begin across frames).

* Stream layer (`Mp3Stream`): ID3v2 skip (with footer), strict resync — a header found while out of sync
  counts only when the next header, a trailing tag (ID3v1 `TAG`, `APETAGEX`) or EOF confirms it;
  Xing/Info frame skipped, its LAME/Lavf tag gives gapless trimming (skip = delay + 529, total =
  frames·spf − delay − max(pad, 529)); channel look-ahead (ISO lets the mode change per frame: stereo if
  any of the first frames is, mono frames duplicated). Unknown-format input (ICY preambles, junk) falls
  back to the MP3 sync scan.
* Rules that were not where one would first look, each found by a failing ISO vector:
  - mixed blocks: the two lowest subbands are long-window (window 0) whatever the block type;
  - LSF intensity stereo: the illegal position is 2^slen − 1 per band (FFmpeg's fixed 16 is wrong for
    small slen), bands with slen 0 are legal at position 0;
  - the top scalefactor band (21 long / 12 short) takes band 20/11's intensity position when that band is
    intensity-coded, else the default (MPEG-1: 3 — no IS; LSF: 0), as ISO and minimp3 do;
  - MS-only frames scale the global gain by 2^−½ (gain − 2);
  - reservoir underflow at stream start: granules whose data lies before what we hold are zeroed (the
    overlap still runs) and decoding resumes at the first whole granule — FFmpeg's rule, and the one that
    took `bear-audio-10s` from 33.1 dB to sample agreement with Chromium;
  - invalid side info (big_values > 288, part2_3 past the frame) drops that granule, never panics.

**Vectors.** The ISO/IEC 11172-4 / 13818-4 Layer III conformance streams (22 bit/pcm pairs: l3-compl,
hecommon, he_32/44/48 kHz, he_free, he_mode, si/si_block/si_huff, sin1k0db, M2L3 bitrate/compl24/noise,
the id3v1/apetag variants), fetched from minimp3's mirror by sha256 (`vectors.txt`, kind `mp3iso`), and
Chromium's `sfx.mp3`, `bear-audio-10s-CBR-has-TOC.mp3`, `id3_png_test.mp3`, `icy_sfx.mp3`,
`midstream_config_change.mp3`.

**Results.** `tests/mp3_iso.rs`: **22/22 streams within 1 LSB of the ISO reference PCM** (RMS ≤ 0.12 LSB;
the reference files omit each stream's last frame, so ours may be one frame longer). Against Chromium
(`lossy_vs_chromium`): `sfx.mp3` 90.1 dB with equal frames; `id3_png_test` bit-identical; `bear-10s` equal
frames, max |d| 3.4e-5 (≈1.1 LSB at 16 bit) — 56.5 dB SNR only because the clip is quiet (rms −41.7 dBFS),
so MP3 passes on SNR ≥ 60 dB **or** every sample within 2 LSB; `icy_sfx` is `sfx.mp3` behind an ICY
preamble — ours gives the same 11,025 frames for both, Chromium gives 14,976 there because it then decodes
the Xing frame and skips the LAME trim; aligned at Chromium sample 2257 (= 1152 + 576 + 529, exactly the
Xing frame plus the gapless skip) it is 90.1 dB. `midstream_config_change` (37 mono frames, then a joint-stereo
one) is refused by Chromium; ours hands it out as stereo (mono duplicated) and its per-frame peaks match
minimp3's, including the stream's own +9 dBFS overs in frames 16–17. Owed: a sample-rate change
mid-stream is handed out at the first rate (no rate-change event in the trait yet).
Robustness: 324 damaged streams (bit flips, truncation, garbage runs) over 27 files, no panic.
Speed: 64x realtime on stereo 44.1 kHz, unoptimised (float IMDCT/polyphase straight from the definitions).

## M4b — AAC-LC (ISO/IEC 14496-3 GA, ISO/IEC 13818-7 ADTS) and the MP4 container

`src/aac/` — float AAC-LC from the standard's syntax (§4.4.2) and tools (§4.6), cross-read with FFmpeg's
and faad2's decoders where the text is terse; the Huffman codebooks, scalefactor-band offsets and TNS band
limits are the standard's tables (taken as data from FFmpeg's aactab.c). raw_data_block with SCE, CPE,
LFE, DSE, PCE, FIL and CCE (parsed and skipped — no encoder in use emits it; applying it is owed); window
grouping, section data, scalefactors (with the 9-bit first noise energy and intensity positions), pulse
data, TNS (coefficients and the step-up recursion computed from §4.6.9.3, all-pole filter up/down), the 11
spectral codebooks with escapes, |q|^(4/3)·2^((sf−100)/4), M/S, intensity stereo (sign from the codebook
and, with `ms_mask_present == 1`, ms_used), PNS (and the correlated noise of §4.6.13.3 when both channels
are noise with ms_used), and the filterbank — the IMDCT is the Vorbis module's (same kernel, n0 = N/4 + ½)
with the 2/N scale, sine and KBD (α = 4/6) windows, all four window sequences, shape switching per half.
Channels come out in WAVE order from channelConfiguration 1–7 or a PCE (front elements centre-outward).

`src/mp4.rs` — ISO-BMFF: the first `soun` track, `stsd` `mp4a`/`.mp3` (QuickTime v1/v2 sample entries,
`esds` also inside `wave`), `stsz`/`stz2`, `stsc`, `stco`/`co64`, fragmented files (`mvex/trex`,
`moof/traf/tfhd/trun`, default-base-is-moof), and gapless trimming from `elst` (media_time, segment
duration rounded to nearest as FFmpeg and faad2 do) or else `iTunSMPB`. Object type 0x40/0x66–0x68 goes to
AAC (an MPEG-2 track without a DecoderSpecificInfo gets one synthesised), 0x69/0x6B to the MP3 decoder.
The file is read whole (the `moov` may come last).

**Oracle.** This Chromium build has no AAC (`decodeAudioData` refuses it), so the reference is **faad2
2.11.4**, a decoder written independently of ours (float build; its frontend's implicit-SBR upsampling
switched off — the streams are plain LC). Vectors: twelve fdk-aac 2.0.3 streams (`oracle/gen-aac.sh`:
8–96 kHz, mono/stereo/3/5.1, 12–256 kb/s CBR + VBR; intensity stereo in four, PNS in three, short
windows in nine) and four MP4 wrappings made by `oracle/adts2mp4.py` (edit list; iTunSMPB with the moov
after the mdat; fragmented; plain), 792 KB with faad's 16-bit decodes stored as FLAC beside them.

* With PNS switched off in both decoders (dev builds), **every stream agrees with faad2 to 132.5–134.4 dB**
  (max |d| ≤ 4.5e-7: float rounding), identical frame counts — the deterministic path is right.
* `tests/aac_kat.rs::aac_vectors_vs_faad2` (committed refs, PNS on): **16/16** — the 12 non-PNS cases at
  77.3–78.8 dB (the 16-bit reference's own limit); the PNS cases by their noise bands, since PNS noise is
  random by design (§4.6.13: any generator): bands where the decodes disagree in waveform must agree in
  level — bias −0.16…+0.16 dB, spread ≤ 1.02 dB (limits 0.5 / 2.5 dB; a 1.5 dB PNS gain error fails it).
* Chromium's AAC samples (fetched, `aac_fetched_streams`): `sfx.adts` 134.3 dB, `bear-audio-lc-aac.aac`
  133.5 dB, `sfx.m4a` (edit list) 134.5 dB vs faad2 with equal frame counts; the fragmented
  `bear-640x360-a_frag.mp4` and `bear-mpeg2-aac-only_frag.mp4` (faad2 cannot open fragments: compared
  through an independent Python demux to ADTS) 133.5 / 133.9 dB. `bear-audio-main-aac.aac` (AAC Main) is
  refused cleanly; the implicit HE-AAC v1/v2 files decode as their LC core at 24 kHz.
* Robustness: 640 damaged ADTS/MP4 files, no panic; sample-table allocations bounded by the file size.
* Speed: 160–170x realtime (stereo 44.1/48 kHz and 5.1), unoptimised.

Owed: SBR and PS (HE-AAC v1/v2: today the LC core at half rate), AAC Main prediction, LTP, CCE application,
960-sample frames, ER/LD/ELD, Opus/FLAC/ALAC in MP4.

## M5 — the kernel's `play <file>` decodes through audio_core

`unaos/crates/kernel` links audio_core by path, **optional, pulled in by `hda-tone` only** (the feature that
already gates `drivers/hda_play.rs`), so a build without the player does not compile it.

* `drivers/hda_play.rs` — five same-line folds, line count unchanged up to the old tail (code first, comments
  after): `open_wav`'s refusal arm becomes `return open_coded(path, r)`, so anything the native WAV parse
  refuses (every non-RIFF file, and 24/32-bit, float, EXTENSIBLE or > 2-channel WAVs) goes to audio_core;
  `service()` gains `coded_pump()` / `coded_report()` beside `wav_pump()` / `report()`; `stop()` drops the
  coded player with the WAV one; the usage line names the formats. Tail-appended: `VfsSrc` (audio_core's
  `Read` over the VFS, 32 KiB forward reads — files stream, nothing is loaded whole except MP4), the
  `Coded` state, `open_coded` (sniff + open + `start(rate/decim, ≤2 ch, 16)`), `coded_pump` (up to four
  4096-frame blocks per tick into the existing `feed()` ring, held back while the FIFO is full; integer
  only: `next_i32` → Q15 downmix to ≤ 2 channels, integer decimation to ≤ 48 kHz, so 88.2/96 kHz play at
  44.1/48), the `:: PLAYCODEC: path= format= codec= rate= frames= lpib_moved= done= -> PASS|FAIL ::`
  witness (frames fed = frames the ring consumed, decoder error-free, frames decoded = the container's
  stated count when it states one, no underrun, LPIB moved, ring drained), and `tests play [fmt]`.
* `tests play` (registered beside `playwav`, same-line in `arch/x86_64/syscall.rs`) plays `TEST.FLAC`,
  `TEST.OPUS`, `TEST.OGG` (Vorbis), `TEST.MP3`, `TEST.AAC`, `TEST.M4A`, `TEST.AIF` from the user's home,
  `/home` or `/`; a missing file prints `:: PLAYCODEC: fmt=<fmt> path=- reason=no-file … -> SKIP ::`.
  `tests play flac` runs one format: `tests.rs` keeps the word after the fixture name (`tests::arg()`,
  one same-line fold in `shell_verb` + a tail accessor).
* x86 metal shape, once, from `unaos/crates/kernel`: `cargo +nightly check --release --target
  ../../x86_64-unaos.json -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem
  -Z json-target-spec --features "wc,quarry,ftdirx,login,loginst,nvidia-kepler-vblank,smc,usbnet,hda,hda-tone,
  facet,beam,sdw,sdwrite,sdhcblk,selfhost,linuxabi,ahci,unafs,busreg,lumen,netring3,prefs_reset,census,
  installdemo,instgui,witness"` → **rc=0**, audio_core checked for the kernel target, no warning in any file
  this arc touched (the 75 warnings are pre-existing, elsewhere); target deleted.
* Not flown: no metal or QEMU run in this arc. The kernel target is `+soft-float` (SSE off), so in the kernel
  FLAC, WAV/AIFF and Opus (the fixed-point decoder) are integer end to end, while MP3, AAC and Vorbis run
  their float maths through compiler-builtins soft-float — whether they keep up in real time inside the
  service tick is the first thing a metal flight must measure (`under=` on the PLAYCODEC line).

`tools/audio-check <file> [--out pcm.wav]` (host): sniff, decode, print what it is (format, codec, rate,
channels, frames vs stated, peak/RMS, decode speed) and write the PCM as a float WAV (the "ears" path —
Chromium can play it back).

## Ceiling and what is owed (whole arc)

Decodes today: WAV (PCM 8–32, float 32/64, A-law/µ-law, EXTENSIBLE), AIFF/AIFF-C, FLAC (native + Ogg), Ogg
Opus (SILK/CELT/hybrid, PLC, FEC; mapping family 0), Ogg Vorbis (floors 0/1, residues 0/1/2), MP3 (MPEG-1/2/2.5
Layer III, ID3v2, Xing/LAME gapless), AAC-LC (ADTS; MP4 plain/fragmented, edit list/iTunSMPB), MP3 in MP4.

Owed: SBR and PS (HE-AAC v1/v2 decode as their LC core at half rate today), AAC Main prediction / LTP / CCE
application / 960-sample frames / ER profiles; Opus multistream (mapping families 1/255, i.e. > 2 channels);
MPEG Layer I/II; ALAC; Opus/FLAC/ALAC in MP4; Ogg chained streams beyond the first logical stream; a
sample-rate change mid-stream (MP3) is handed out at the first rate; speed work (every float codec is
unoptimised: 37–170x realtime on the host); the kernel leg's metal flight (soft-float real-time margin).
