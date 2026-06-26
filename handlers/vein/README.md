# vein

**Vein is the AI handler for UnaOS** — the domain service that turns user prompts into model responses, manages conversational memory, and assembles the context sent to the language model.

It is the reference implementation behind Lumen's chat experience. Like every UnaOS handler, Vein is a self-contained crate that communicates only over the Bandy message bus (`SMessage` on the `Synapse`); it never calls other handlers directly.

## Status

**Implemented (partial).** The prompt → retrieve → generate → persist pipeline, file upload, AST skeletonization, and Matrix topology integration all work today. Some adjacent machinery (the `GravityWell` context-scoring model, `CortexStorage`) is built but not yet wired into the live request path.

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

Storage and persistence (the vector database) are owned by a separate handler; Vein reaches them only through these messages.

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
