# VEINPROV — Vein talks to the provider the person configured (ledger B303)

## Design

**Finding.** Peter, 2026-10-04: "getting vein to have the ability to connect to gemini which it is
currently hard wired for, and/or claude". `libs/gneiss_pal/src/api/mod.rs` was the one model client
and it was hardwired to Google Vertex Gemini: a literal project id, a literal model
(`ResilientClient::new`), a second literal Vertex URL for embeddings, and `list_vertex_models`
answering "Hardcoded to ...". Every Vein model call went through it. ROADMAP SH-4 charters the
opposite: "no provider is hardwired".

**Seam.** Vein owns the provider abstraction (CODEX: Vein = AI, "Provider Abstraction
(Local/Cloud)"); the code lives in `gneiss_pal::api` because that is the crate Vein already links for
its client, and Vein is its only model consumer. Principia owns the settings surface: the provider is
read from Principia's preference store (namespace `vein`) through `principia::prefs::PrefStore`, the
same TOML the kernel writes through `prefs_core`. Holocron owns credentials: it is design-only, so the
API key is never in the preference file; the preference NAMES an environment variable and the key is
read from it (Holocron takes custody when it exists). Host-native Ring-3 arc: no kernel file, so
GATE-CHARTER is not in scope.

**Milestones.**
- M1 — `gneiss_pal::api::provider`: `trait ModelProvider` (`generate`, `stream`, `embed`, `name`,
  `model`), provider-neutral `ChatRequest`/`ChatMessage`/`Role`/`ChatResponse`/`StopReason`/`Usage`/
  `ChatDelta`/`ProviderError`, the shared retry policy (`api::retry`, which Vein's `SynapticRetry`
  now delegates to), and the SSE line decoder.
- M2 — `GeminiProvider`: the old client behind the trait; project, region, model, embedding model and
  auth mode (`gcloud` ADC token or an API key from a named env var) are constructor configuration.
- M3 — `ClaudeProvider`: raw HTTP `POST /v1/messages` with `reqwest`, `anthropic-version: 2023-06-01`,
  default model `claude-opus-5-5`, no `thinking` field, no prefill, SSE streaming, `stop_reason`
  checked before content, refusal mapped, server-side fallbacks on by default, retryable/non-retryable
  status mapping with `retry-after`. Mock-server unit tests, no live network.
- M4 — `ProviderConfig::from_prefs` + `build_provider`; Vein builds the provider at start and again on
  `PrincipiaCommand::PrefChanged{ns:"vein"}`; a missing key is an in-chat error, never a panic and
  never a silent Gemini fallback.
- M5 — Lumen/quartzite: the New Node model dropdown lists `gneiss_pal::api::MODEL_CHOICES` and
  preselects the configured model; Vein prints the configured provider/model on connect.

**Witness.** Host tests: `cargo test -p gneiss_pal -p vein`; `cargo check -p lumen`. The live line in
Lumen's console on connect: `:: BRAIN :: ONLINE (claude / claude-opus-5-5)` (or the configured pair),
and with no key `:: BRAIN :: NO PROVIDER :: set ANTHROPIC_API_KEY, or choose a provider in Settings`.

**Owed.** `tools/foreman/src/advisor/provider.rs` is a scaffold provider trait that says it migrates onto this one at the Vein rung — that migration is a follow-up. Embeddings with Claude (Anthropic has no embeddings endpoint: semantic recall degrades to
an empty query vector unless a Gemini embedder is configured — an `vein.embed.*` provider split is
owed); nothing in Vein consumes `stream` yet (both providers implement it over SSE; the chat path still calls `generate`); attachments are Gemini `gs://` URIs
from the upload service, which Claude cannot fetch (Claude gets a URL document/image block for an
`https://` URI and a clear refusal-to-attach error otherwise); the quartzite dropdown is GTK-gated and
not compiled on a host without GTK4; Holocron custody of the key.
