# EMBED — the embedder is its own setting (ledger B317)

## Design

**Finding.** R81 (Peter, 2026-10-04): Claude is Vein's default chat provider, and the embedder is its
own setting, independent of the chat provider. VEINPROV (B303) left embedding as an optional method on
the chat trait (`ModelProvider::embed`): Anthropic has no embeddings endpoint, so with the default
provider every vault store wrote an EMPTY vector and semantic recall silently did nothing. The vault
(`handlers/vein/src/vault.rs`) also stored a bare `embedding` vector with no record of which model
made it, so a model change would compare vectors from two different spaces.

**Seam.** Vein owns it (CODEX §2: Vein = AI, "Provider Abstraction (Local/Cloud)"). The trait lives in
`gneiss_pal::api` beside `ModelProvider` (the crate Vein links for its client); the setting is
Principia's (`vein` namespace); the vector store is the vault Vein already owns on UnaFS. Host-native
Ring-3 arc: no kernel file, so no CHARTER line is required (GATE-CHARTER does not apply).

**Milestones.**
- M1 — `gneiss_pal::api::embed`: `trait Embedder` (`embed(texts) -> Vec<Vec<f32>>`, `dims`, `name`,
  `model`), `GeminiEmbedder` (the embedding call moves OFF `GeminiProvider`; the chat trait no longer
  embeds), `NoEmbedder` (dims 0, writes no vector). `EmbedConfig::from_prefs`: `embed.provider` ∈
  `gemini|local|off`, default `gemini` when the `gemini.api_key_env` variable is set or
  `gemini.project` is configured, else `off`; `embed.model`, `embed.dims`. Vein prints
  `:: BRAIN :: EMBED <provider>/<model> dims=<n>` at start and on a `vein` pref change; with no
  embedder recall says `:: BRAIN :: RECALL OFF :: no embedder — set vein.embed.provider`. The vault
  writes `una:embed-model` + `una:embed-dims` beside each vector; a query only compares vectors of
  the configured model; others are counted as stale and printed, never compared.
- M2 — `LocalEmbedder` (all-MiniLM-L6-v2, 384 dims, mean-pooled, L2-normalised) on candle, CPU;
  hand-written BERT WordPiece tokenizer and a minimal ONNX initializer reader (no protoc). Model files
  are fetched by `tools/una-models` into `~/.cache/unaos/models/<name>` with sha256 pins; absent files
  are an in-chat message naming the command. A 20-sentence golden test (reference: onnxruntime +
  HF tokenizers on the same files) runs only when the files are present.
- M3 — re-embed: the `/reembed` chat verb drives bus messages `ReEmbed` → `ReEmbedBatch` →
  `ReEmbedWrite` → `ReEmbedDone`, bounded per pass, with `:: BRAIN :: REEMBED` progress lines. Lumen's
  settings dialog shows `claude / claude-opus-5-5 · embed gemini/text-embedding-004`.
- M4 — docs: Vein README embedder section, ROADMAP SH-4 clause, ledger B317.

**Crate decision (evidence, 2026-10-04, this container).** Order evaluated: candle → tract →
hand-written. `candle-core` + `candle-nn` + `candle-transformers` 0.11.0 BUILD here (crates.io is
reachable; scratch crate, `cargo build` exit 0, 165 crates). `candle-onnx` does NOT build (its
build script needs `protoc`; not installed and `deb.debian.org` is refused by the egress proxy).
The canonical all-MiniLM-L6-v2 `model.safetensors` on huggingface.co is refused by the egress proxy
(CONNECT 403), so its sha256 cannot be pinned from here. The reachable pinned source is Chroma's
published ONNX export of the same model (`chroma-onnx-models.s3.amazonaws.com/all-MiniLM-L6-v2/
onnx.tar.gz`, sha256 `913d7300ceae3b2dbc2c50d1de4baacab4be7b9380491c27fab7418616a16ec3`).
So: candle runs `BertModel`; its weights are read straight out of the ONNX file's initializers by a
small protobuf reader (the six per-layer MatMul weights carry `onnx::MatMul_*` names and are mapped
to their BERT names through the `Add` node that adds the named bias, then transposed).

**The metal design (not shipped).** A `no_std` MiniLM forward pass over int8 weights: the tokenizer
and the ONNX reader in this arc are already `core`+`alloc` shaped (no I/O, no std collections beyond
`BTreeMap`); the forward pass is 6 layers × (QKV 384×1152, out 384×384, FFN 384×1536×384) with
per-row int8 scales (≈ 22 MB weights → ≈ 6 MB), erf-GELU by a 7th-order polynomial, LayerNorm with
`f32` accumulation, mean pooling, L2 norm. The weights file would be produced on the host from the
same pinned ONNX by `una-models` and loaded from UnaFS on the metal.

**Witness.** `cargo test -p gneiss_pal -p vein -p principia`, `cargo check -p lumen`. Lumen console
on connect with the defaults and no Gemini key:
`:: BRAIN :: EMBED off/none dims=0` then `:: BRAIN :: RECALL OFF :: no embedder — set vein.embed.provider`.

**Owed.** The canonical HF safetensors pin; the no_std metal forward pass; Holocron custody of keys.

## Results (2026-10-04)

- M1 `b7583b08` — `Embedder`, `GeminiEmbedder`, `NoEmbedder`, `EmbedConfig::from_prefs`, `EmbedSlot`
  in Vein, tagged vault vectors, the `ReEmbed*` bus messages (with KATs) and the re-embed driver
  (`handlers/vein/src/reembed.rs`; it landed in this commit, M3 only added the label).
- M2 `2903a303` — `LocalEmbedder` (`libs/gneiss_pal/src/api/local/{mod,onnx,wordpiece}.rs`),
  `tools/una-models` (fetch/verify/list, sha256 pins on the archive and on each file), the golden
  test `libs/gneiss_pal/tests/local_embed_golden.rs` + fixture.
- M3 `e061b205` — Lumen settings dialog label (`libs/quartzite/src/platforms/gtk/workspace/sidebar.rs`).
- M4 — this file's results, Vein README embedder section, ROADMAP SH-4 clause, ledger B317.

Gates (exit codes, repo root): `cargo test -q -p gneiss_pal -p vein -p principia -p bandy` → 0
(gneiss_pal 42 lib + 1 golden, vein 16, principia 19, bandy 102 KATs); `cargo test -q -p gneiss_pal`
alone (feature off) → 0; `cargo check -q -p lumen` → 0; `charter-check.sh` → 0.
Golden: `minilm golden: 20/20 token-exact, worst cosine to reference 0.9999996`; with no model:
`SKIP minilm golden: all-MiniLM-L6-v2 is not installed in /nonexistent/unaos/models/all-MiniLM-L6-v2 — run \`tools/una-models fetch all-MiniLM-L6-v2\``.
`tools/una-models fetch all-MiniLM-L6-v2` → 0, `verify` → 0 (three `ok` lines).

Not compile-checked here: the quartzite `gtk` feature (no GTK4 on this container), so the dialog
label is written but its compile is owed to a GTK4 host.
