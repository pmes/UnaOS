# vein

**Vein is the AI handler for UnaOS** — the domain service that turns user prompts into model responses, manages conversational memory, and assembles the context sent to the language model.

It is the reference implementation behind Lumen's chat experience. Like every UnaOS handler, Vein is a self-contained crate that communicates only over the Bandy message bus (`SMessage` on the `Synapse`); it never calls other handlers directly.

## Status

**Implemented (partial).** The prompt → retrieve → generate → persist pipeline, file upload, AST skeletonization, and Matrix topology integration all work today. Vein also owns its own durable **Semantic Vault** (`vein::vault`), the UnaFS-backed engram store that serves `StorageSave`/`StorageQuery`/`StorageLoadPaged` over the bus. Some adjacent machinery (the `GravityWell` context-scoring model, `CortexStorage`) is built but not yet wired into the live request path.

## Responsibilities

- Receive user input, build a system prompt from retrieved context, call the LLM, and stream the result back onto the bus.
- Maintain conversational memory: persist user/model turns and compress each exchange into a dense **engram** for long-term recall.
- Index the workspace into token-efficient **skeletons** (function bodies stripped from the AST) and supply them, plus live Matrix code topology, as model context.
- Handle file uploads (multipart POST to the Vein upload service) and rewrite them into multimodal `[ATTACHMENT:mime|uri]` prompt parts.

## Entry point

`VeinHandler::new(history_path, synapse, app_state, shutdown_tx) -> (VeinHandler, JoinHandle)`

The constructor spawns a background **brain loop** (a Tokio task) that subscribes to the `Synapse`, brings up the configured model provider (`vein::provider::ProviderSlot` over `gneiss_pal::api::ModelProvider` — see "Model provider" below) and an optional `ForgeClient`, kicks off workspace indexing, and then services bus events and queued user input until shutdown. `VeinHandler` itself implements `bandy::AppHandler` (synchronous `handle_event`) and `bandy::BandyMember` (publish).

## Model provider (VEINPROV)

Vein talks to the provider the person configured — Claude or Gemini today, any later one behind the same trait (`gneiss_pal::api::ModelProvider`). Nothing is hardwired (ROADMAP SH-4). Vein owns the abstraction; **Principia** owns the setting; **Holocron** will own the credential.

The choice is read from Principia's preference store (`~/.config/unaos/preferences.toml`, the file the kernel writes through `prefs_core`), namespace `vein`, at start and again whenever Principia broadcasts `PrefChanged` for that namespace:

| Key | Values | Default |
| :--- | :--- | :--- |
| `vein.provider` | `"claude"` \| `"gemini"` | `"claude"` |
| `vein.model` | any model id | `claude-opus-5-5` (Claude), `gemini-3.1-pro-preview` (Gemini) |
| `vein.max_tokens` | int | 16000 (Claude); unset sends no cap to Gemini |
| `vein.temperature` | float | provider default (Gemini 0.4) |
| `vein.claude.api_key_env` | env var NAME | `ANTHROPIC_API_KEY` |
| `vein.claude.fallbacks` | bool | `true` |
| `vein.gemini.auth` | `"gcloud"` \| `"api_key"` | `"gcloud"` |
| `vein.gemini.project` | Vertex project id | none — required for `gcloud` |
| `vein.gemini.region` | Vertex location | `global` |
| `vein.gemini.api_key_env` | env var NAME | `GEMINI_API_KEY` |

```toml
[vein]
provider = "claude"

[vein.claude]
api_key_env = "ANTHROPIC_API_KEY"
```

**The API key is never written to the preference file.** The preference names an environment variable and the key is read from it. Holocron takes custody of the credential when it exists (design-only today). A missing key, a Gemini `gcloud` setup with no project, or an unreadable preference file is an in-chat error naming the fix (`:: BRAIN :: NO PROVIDER :: set ANTHROPIC_API_KEY, or choose a provider in Settings`) — never a panic and never a silent fallback to another provider. With a provider up the console reads `:: BRAIN :: ONLINE (<provider> / <model>)`.

**Claude** (`gneiss_pal::api::ClaudeProvider`): raw HTTP to `POST https://api.anthropic.com/v1/messages` (`anthropic-version: 2023-06-01`). No `thinking` field is sent (adaptive thinking is the model default), no assistant prefill, `stop_reason` is checked before the text (a `refusal` shows as `[declined by the provider: <category>]`). **Server-side fallbacks are on by default** (`anthropic-beta: server-side-fallback-2026-07-01` and `"fallbacks": "default"`); set `vein.claude.fallbacks = false` to turn them off. Attachments: an `https://` PDF or image goes as a URL `document`/`image` block; a `gs://` upload from the Vein upload service is Gemini-only and is refused with an in-chat error, never dropped silently. Anthropic has no embeddings endpoint; the embedder is a separate setting (below).

**Gemini** (`gneiss_pal::api::GeminiProvider`): `gcloud` mode talks to Vertex AI with an ADC token (refreshed once on a 401); `api_key` mode talks to the Generative Language API with `x-goog-api-key`. The chat provider does not embed: see the embedder.

Every provider sends through the one backoff (`gneiss_pal::api::retry`, which `SynapticRetry` now delegates to): 408/409/429/5xx and connection errors are retried with exponential backoff and jitter, a `retry-after` is honoured; 400/401/403/404 are final.

## Embedder (EMBED)

**The embedder is its own setting, independent of the chat provider** (R81, Peter 2026-10-04). Every vector the vault stores comes from `gneiss_pal::api::Embedder` (`embed(texts)`, `dims`, `name`, `model`); the chat trait has no `embed`. Three embedders:

- **`gemini`** — `GeminiEmbedder`: Gemini's embedding model, Vertex `predict` under gcloud ADC or `embedContent` under an API key (the same `vein.gemini.*` auth keys the chat provider uses).
- **`local`** — `LocalEmbedder` (gneiss_pal feature `local-embed`, on in Vein by default): all-MiniLM-L6-v2 (BERT, 384 dims, mean-pooled, L2-normalised) run in-process on the CPU by candle — no network. The model files are not in the repository: `tools/una-models fetch all-MiniLM-L6-v2` puts them in `~/.cache/unaos/models/all-MiniLM-L6-v2/` (sha256-pinned; `tools/una-models verify <name>` re-checks). A missing model is an in-chat line naming that command, never a panic.
- **`off`** — `NoEmbedder` (dims 0): every store writes no vector and recall is OFF, said in-chat: `:: BRAIN :: RECALL OFF :: no embedder — set vein.embed.provider`.

| Key | Values | Default |
| :--- | :--- | :--- |
| `vein.embed.provider` | `"gemini"` \| `"local"` \| `"off"` | `"gemini"` when the `vein.gemini.api_key_env` variable (default `GEMINI_API_KEY`) is set or `vein.gemini.project` is configured; else `"off"` |
| `vein.embed.model` | embedding model id | `vein.gemini.embed_model`, else `text-embedding-004` (gemini); `all-MiniLM-L6-v2` (local) |
| `vein.embed.dims` | int | the model's known width (768 / 384); a vector of another width is refused |
| `vein.embed.reembed_batch` | int | 16 memories per `/reembed` pass |
| `vein.gemini.embed_region` | Vertex location for embeddings | `us-central1` |

At start (and on a `vein` preference change) the console reads `:: BRAIN :: EMBED <provider>/<model> dims=<n>` — e.g. `:: BRAIN :: EMBED local/all-MiniLM-L6-v2 dims=384`, or `:: BRAIN :: EMBED off/none dims=0` followed by the recall-off line.

**Vectors never compare across models.** The vault writes `una:embed-model` (`<provider>/<model>`) and `una:embed-dims` beside each `embedding` vector; a query compares only vectors with the configured tag. Memories made by another model, untagged ones from before EMBED, and ones stored while recall was off are counted at start and printed (`:: BRAIN :: VAULT <n> memories not embedded by <tag> — never compared; /reembed re-embeds them`). Typing **`/reembed`** walks them onto the configured embedder in bounded passes (`ReEmbed` → `ReEmbedBatch` → `ReEmbedWrite` → `ReEmbedDone`), printing `:: BRAIN :: REEMBED <done>/<total> (<n> stale left)` per pass.

Lumen's settings dialog names both halves: `claude / claude-opus-5-5 · embed gemini/text-embedding-004`.

## Bus interface (`SMessage`)

**Consumes** — UI/input events via `handle_event`: `Input`, `ComplexInput`, `DispatchPayload`, `LoadHistory`, `FileSelected`, `UpdateMatrixSelection`, `TemplateAction`, `NavSelect`, `ToggleSidebar`. Bus events via the brain loop's subscription: `TriggerUpload`, `Principia(PrefChanged{ns:"vein"})` (rebuild the provider and the embedder), `StorageQueryResult`, `StorageLoadPagedResult`, `StorageSaveResult`, `ReEmbedBatch`, `ReEmbedDone`, and `Matrix(MatrixEvent::{IngestTopology, SectorFocused, GraftTopology})`.

**Emits**: `StorageQuery` and `StorageLoadPaged` (request memory), `StorageSave` (persist turns and engrams; both carry `embed_model`), `ReEmbed` / `ReEmbedWrite` (the re-embed walk), `ContextTelemetry` (ranked skeletons), `NetworkState` (in-flight indicator), `TriggerUpload`, `Log`, and `StateInvalidated` to prompt the GUI to repaint.

Vein serves its own durable memory: the **Semantic Vault** actor (`vein::vault::ignite`) holds an exclusive lock on one UnaFS volume and answers `StorageSave` / `StorageQuery` / `StorageLoadPaged` / `ReEmbed` / `ReEmbedWrite` (replying with `StorageSaveResult` / `StorageQueryResult` / `StorageLoadPagedResult` / `ReEmbedBatch` / `ReEmbedDone`). The brain loop and the vault actor communicate only through these bus messages, never by direct call.

## The Semantic Vault (`vein::vault`)

`vault.rs` is vein's durable engram store. Its host app (Lumen) spawns it with:

```rust
pub async fn ignite(vault_path: PathBuf, synapse: Synapse)
```

On startup `ignite` mounts (or, on true first run, formats) the UnaFS vault on a blocking thread, then runs an actor loop over `synapse.subscribe()`. Because UnaFS I/O is synchronous and blocking, every request is dispatched to `tokio::task::spawn_blocking` and the owned `DiskManager` is moved in and out of the blocking task — it is never driven on the async reactor thread.

**AMBER-GUARD (fail-closed mount).** If an existing vault file cannot be mounted (corruption, version skew, transient I/O), `DiskManager::new` returns the error and leaves the on-disk bytes **byte-identical** — never truncated, never reformatted — so the data can be recovered. This data-loss guard is covered by the `vault::tests` byte-identity tests and is non-negotiable.

## Modules

| Module | Role |
| --- | --- |
| `lib.rs` | `VeinHandler`, the brain loop, request assembly, upload, multimodal parsing. |
| `skeleton.rs` | `SkeletonGenerator` — parses Rust with `syn` and strips function bodies for token efficiency. |
| `cortex.rs` | Workspace indexer; memory-maps each source file and skeletonizes it. |
| `context.rs` | `compress_into_engram` — LLM-driven compression of a conversation turn. |
| `gravity.rs` | `GravityWell` — relevance scoring of skeletons (focus / activity / keyword signals). |
| `synapse.rs` | `SynapticRetry` — exponential backoff with jitter for the model endpoint. |
| `storage.rs` | `CortexStorage` — on-disk paths for models and the memory database. |
| `vault.rs` | The Semantic Vault: `DiskManager` (UnaFS engram store) + the `ignite` storage actor + the AMBER-GUARD fail-closed mount tests. |
