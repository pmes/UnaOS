# INFERCORE — UnaOS runs its own inference (ledger SR57)

## Finding

Vein's local embedder (EMBED, B317: all-MiniLM-L6-v2 on the CPU) ran on the three `candle` crates
(DEPS SR31: OWED, chicken wire) with `unicode-normalization` for NFD: the only inference UnaOS did was
done by someone else's tensor library. Its hand-written WordPiece also disagreed with the reference
tokenizer on 3 of the 100 oracle sentences (found by M3).

## What was built

`unaos/libs/ml/infer_core` — `#![no_std]` + `alloc`, `#![forbid(unsafe_code)]`, **zero dependencies**
(type-checks for `aarch64-unknown-none-softfloat`). A member of the root workspace.

| Module | Specification | What it does |
|---|---|---|
| `safetensors` | huggingface/safetensors "Format" | 8-byte LE header length, JSON header, raw LE tensors; validation as the reference loader (≤ 100 MB header, length = numel × dtype size, tensors tile the buffer exactly, no duplicates); every dtype listed, F32/F16/BF16 → f32 |
| `onnx` | onnx.proto3 (initializers + nodes) | hand-written protobuf walk; `MatMul`→`Add(bias)` renaming gives exported weights their BERT names (moved here from gneiss_pal) |
| `json` | RFC 8259 | strict reader; numbers keep their text (exact u64 offsets); depth cap 64 |
| `unicode` + `unicode_tables` (generated) | UAX #15, UCD | NFD (full canonical decomposition, Hangul §3.12, canonical ordering); Cc/Cf/Co, Mn, P* predicates |
| `tokenizer` | HF `BertTokenizerFast` | `tokenizer.json` added tokens (leftmost-longest in the raw text; `normalized`/`lstrip`/`rstrip`/`single_word`), BertNormalizer, BertPreTokenizer, WordPiece, `[CLS] … [SEP]` + truncation; also `vocab.txt` with the BERT defaults |
| `bert` | BERT (Devlin et al.) + HF `BertModel` + sentence-transformers pooling | embeddings `(word + type) + pos` → LN; N post-LN layers: fused QKV, per-head softmax(q·k/√d) over the SAME sequence only, erf GELU, LayerNorm with eps; masked mean + `x / max(‖x‖, 1e-12)`; packed batches (no padding) |
| `matmul` | — | `nn.Linear` with weights packed in 8-column panels, k-blocks of 256, 4×8 register block; f32 or binary16 weights (widened per block) |
| `math`, `half` | IEEE 754, fdlibm `s_erf.c` | own `exp` (Cody–Waite + degree-13), `sqrt` (Newton + exact-residual correction), `erf`/`erfc` (fdlibm rational forms, coefficients carried with Sun's notice, cross-checked against an independent series and continued fraction), f16/bf16 ↔ f32 (RNE) |

**Why the Unicode data is 8.0 and 9.0.** Byte-equal ids mean the reference's data: HF `tokenizers`
takes its general categories from `unicode_categories` 0.1.1 (UnicodeData.txt 8.0.0; private use as
full ranges) and its NFD from `unicode-normalization-alignments` 0.1.12 (9.0.0). `tools/gen_unicode.py`
generates `src/unicode_tables.rs` (data only) from exactly those two files (pinned in `vectors.txt`);
whitespace and lowercase come from `core`, as in the reference. The reference's CJK range typo
(`0x2B920`, not `0x2B820`) is kept on purpose.

**Order of operations (reproducibility).** Dense layers sum in f32 in ascending `k` then add the bias
(blocking never changes the rounding sequence; Rust never contracts to FMA); `q·k` and `Σ p·v` in f32
ascending; LayerNorm statistics, softmax and pooling in f64, rounded once; transcendental functions are
ours. So the output bits do not depend on the machine, and a sequence's vector is the same bits alone
or in any batch position (tested).

**Face.** `gneiss_pal::api::local::LocalEmbedder` (feature `local-embed` keeps its name) reads
`config.json`, `model.safetensors` (F32/F16/BF16) or else `model.onnx`, `tokenizer.json` or else
`vocab.txt`, truncates at 256 tokens and splits a batch across the CPU's threads. `local-embed =
["std", "dep:infer_core"]`. `tools/una-models` now also installs and pins `tokenizer.json`.
`cargo tree -p gneiss_pal -e normal --features local-embed` shows `infer_core` and no candle / gemm /
safetensors / unicode-normalization; candle is gone from `Cargo.lock` (`unicode-normalization` stays
in the lock only as `gix-utils`' dependency).

No new handler: embedding stays Vein's (CODEX §2, "Provider Abstraction (Local/Cloud)").

## Oracles

| Oracle | Method | Result |
|---|---|---|
| Tokenizer | HF `tokenizers` 0.23.2 ids on 2000 generated strings (English, accents + combining marks, Greek final sigma, Cyrillic, Arabic, Hebrew, Indic, Thai, CJK incl. the 0x2B820 gap, Hangul, kana, emoji/ZWJ/flags, controls, format chars, private use, unassigned, exotic whitespace, specials in text, > 100-char words, random code points over all planes); `kat/tokenizer_kat_{0,1}.txt`, sha-pinned | **2000/2000 byte-equal**, from `tokenizer.json` and from `vocab.txt` |
| safetensors reader | `kat/reader_kat.safetensors` written by HF `safetensors` 0.8.0 (F32, F16, BF16, I64, scalar), expected f32 bits from numpy / ml_dtypes in its metadata | bit-exact |
| real F16 model file | all-MiniLM-L6-v2 `model.fp16.safetensors` (npm `@lat.md/embed-minilm-fp16` 0.1.0, pinned) vs the ONNX f32 weights, tensor by tensor | 101 tensors: 22 564 015 values == RNE f16; 1361 exact ties rounded away from zero by that file's writer (numpy agrees with ours) |
| encoder vs onnxruntime | `kat/minilm_ort.f32`: HF tokenizers (trunc 256) → onnxruntime 1.30.0 on the same pinned `model.onnx` → masked mean → normalise in f32 (sentence-transformers' recipe), 100 sentences (1515 tokens, two at 256) | **worst cosine 1.000000000, worst max abs diff 4.4e-7** (gate ≥ 0.9999 / ≤ 1e-4) |
| encoder vs candle | `kat/minilm_candle.f32`: candle 0.11 (the shipped path) on the same ids, recorded once by `libs/gneiss_pal/tests/infer_core_vs_candle.rs` (commit `873f2ece`) before M4 removed candle | **worst cosine 1.000000000, worst max abs diff 3.8e-7** |
| batching | batch 16 vs batch 1, and reversed order | bit-identical |
| f16 weights | rounded at load / the F16 file | worst cosine 0.9999995 / 0.9999989, max abs diff 1.8e-4 / 3.2e-4 (gate ≥ 0.999 / ≤ 5e-3) |
| EMBED golden (B317) | the existing 20-sentence fixture through the new face | 20/20 token-exact, worst cosine 0.9999996 |
| fuzz | 20 000 mutants each of safetensors, ONNX, tokenizer.json, vocab.txt, config.json (bit flips, boundary bytes, truncation, span duplication, header-length corruption, junk), 5000 random blobs, real-file heads | no panic |

Timing (ms per sentence over the 100 oracle sentences, this container: 4 vCPU Xeon @ 2.1 GHz shared
with other sessions — indicative, not a benchmark):

| Path | batch 1 | batch 16 |
|---|---|---|
| candle 0.11 (CPU, its thread pool; batch padded to the longest) | 20.6 | 67.0 |
| infer_core, 1 thread (packed batch) | 35.7 | 23.7 |
| gneiss_pal face (infer_core, batch split over 4 threads) | 28.9 | 16.7 |

The kernel is portable Rust auto-vectorised at the x86-64 baseline (SSE2): 16.7 GFLOP/s single-thread
on a 256×384×1536 layer.

KATs: 20 unit tests (json, half incl. all 65 536 halves round-trip, unicode/NFD, tokenizer, safetensors,
matmul bit-identity to the documented order for f32 and f16, math vs references and cross-checks, bert
batch/shape/error cases) + 7 integration tests (tokenizer KAT, reader KAT, F16 provenance, oracle, 3 fuzz).

Python was used ONCE, off the build, to record the reference files (`kat/record.py`): tokenizers 0.23.2,
onnxruntime 1.30.0, numpy 2.4.6, safetensors 0.8.0, ml_dtypes 0.6.0. No Rust third-party crate is
under infer_core (dev-dependency: `crypto_core`, UnaOS's own, for the sha256 pins).

## Gates

`cargo test --release -p infer_core -p gneiss_pal --features local-embed` → exit 0 (gneiss_pal 81 lib +
5 + golden; infer_core 20 lib + 7 integration). Vectors are fetched at test time into
`target/infer_core-vectors/` (override `INFER_CORE_VECTORS`), sha256-checked before unpacking; offline →
SKIP lines. `cargo check -p vein --all-features` → 0. `cargo check -p infer_core --target
aarch64-unknown-none-softfloat` → 0.

## Honest ceiling

- Model family: BERT encoders only (absolute positions, erf GELU, post-LN); WordPiece + BertNormalizer
  + BertPreTokenizer only (other `tokenizer.json` pipelines are refused with an error).
- The ONNX reader reads initializers and the node list; it does not execute graphs.
- No SIMD intrinsics or runtime CPU dispatch (the crate forbids `unsafe`); single-thread batch 1 is
  slower than candle's multi-threaded gemm. No int8.
- References: sentence-transformers itself needs torch, which this container cannot install (no
  download.pytorch.org; the PyPI wheel pulls GBs of CUDA); the reference is its recipe on
  onnxruntime + HF tokenizers. huggingface.co is refused by the egress proxy, so the canonical F32
  `model.safetensors` is not pinned; the F32 safetensors path is proven on HF's writer output (KAT) and
  the model-scale safetensors path on the provenance-checked F16 file.
- The kernel does not link infer_core yet (Ring 0 embedder and loading weights from UnaFS are owed).

## Owed

- Ring 0: link infer_core from the kernel, load the model from UnaFS (the EMBED "metal design").
- SIMD kernels (AVX2/AVX-512, NEON) behind a safe dispatch seam; int8 weights.
- Pin the canonical HF `model.safetensors` (and record a torch sentence-transformers reference) from a
  host with huggingface.co / PyTorch access.
- DEPS.md rows for `candle-core`/`candle-nn`/`candle-transformers` (SR31) → retired at the fold.

## Continue

Re-record references: `PYTHONPATH=<site> python3 unaos/libs/ml/infer_core/kat/record.py <dir with
tokenizer.json + model.onnx>` then re-pin the sha256s in `tests/{tokenizer_kat,oracle,safetensors_kat}.rs`.
Regenerate Unicode data: `tools/gen_unicode.py UnicodeData-8.0.0.txt UnicodeData-9.0.0.txt >
src/unicode_tables.rs` (files in `vectors.txt`). A new BERT-family model: drop `config.json`,
`tokenizer.json` and `model.safetensors` into `~/.cache/unaos/models/<name>` and set `vein.embed.model`.

## Results (2026-10-05)

- M1 `1a8a5a04` — crate, readers, tokenizer (2000/2000), UCD data generator.
- M2 `c154c82a` — encoder vs onnxruntime (worst max abs diff 4.4e-7), kernel, own math.
- M3 `873f2ece` — candle oracle recorded + pinned, timing, fuzz.
- M4 `13ff44bf` — the face on infer_core; candle and unicode-normalization out of gneiss_pal.
- M5 — this file; Vein README.
