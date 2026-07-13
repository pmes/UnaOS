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

The constructor spawns a background **brain loop** (a Tokio task) that subscribes to the `Synapse`, brings up the LLM client (`gneiss_pal::ResilientClient`) and an optional `ForgeClient`, kicks off workspace indexing, and then services bus events and queued user input until shutdown. `VeinHandler` itself implements `bandy::AppHandler` (synchronous `handle_event`) and `bandy::BandyMember` (publish).

## Bus interface (`SMessage`)

**Consumes** — UI/input events via `handle_event`: `Input`, `ComplexInput`, `DispatchPayload`, `LoadHistory`, `FileSelected`, `UpdateMatrixSelection`, `TemplateAction`, `NavSelect`, `ToggleSidebar`. Bus events via the brain loop's subscription: `TriggerUpload`, `StorageQueryResult`, `StorageLoadPagedResult`, `StorageSaveResult`, and `Matrix(MatrixEvent::{IngestTopology, SectorFocused, GraftTopology})`.

**Emits**: `StorageQuery` and `StorageLoadPaged` (request memory), `StorageSave` (persist turns and engrams), `ContextTelemetry` (ranked skeletons), `NetworkState` (in-flight indicator), `TriggerUpload`, `Log`, and `StateInvalidated` to prompt the GUI to repaint.

Vein serves its own durable memory: the **Semantic Vault** actor (`vein::vault::ignite`) holds an exclusive lock on one UnaFS volume and answers `StorageSave` / `StorageQuery` / `StorageLoadPaged` (replying with `StorageSaveResult` / `StorageQueryResult` / `StorageLoadPagedResult`). The brain loop and the vault actor communicate only through these bus messages, never by direct call.

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
