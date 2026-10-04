// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! The Gemini provider (VEINPROV, B303): the old Vertex `ResilientClient`
//! behind [`ModelProvider`], with nothing hardcoded — project, region, model,
//! embedding model and auth mode are constructor configuration.
//!
//! Two auth modes:
//! - [`GeminiAuth::GcloudAdc`] — Vertex AI
//!   (`{region}-aiplatform.googleapis.com/v1beta1/projects/{project}/locations/{region}/...`,
//!   `global` → `aiplatform.googleapis.com`), bearer token from
//!   `gcloud auth application-default print-access-token`, refreshed once on a
//!   401 (the "Lazarus" retry the old client had).
//! - [`GeminiAuth::ApiKey`] — the Generative Language API
//!   (`generativelanguage.googleapis.com/v1beta/models/{model}`), header
//!   `x-goog-api-key`, no project.
//!
//! Attachments: [`super::Part::FileData`] is Gemini's own shape and passes through.

use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use log::{error, info, warn};
use reqwest::{Client, ClientBuilder, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::provider::{
    BoxFuture, ChatDelta, ChatRequest, ChatResponse, DeltaStream, GeminiSettings, f32_json, ModelProvider, ProviderError, Role,
    StopReason, Usage,
};
use super::embed::{Embedder, check_batch};
use super::retry::{RetryPolicy, send_classified};
use super::sse::{SseEvent, SseHandler, sse_stream};
use super::Content;

/// The temperature the old client always sent; kept as Gemini's default.
pub const GEMINI_DEFAULT_TEMPERATURE: f32 = 0.4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeminiAuth {
    GcloudAdc,
    ApiKey(String),
}

#[derive(Debug, Clone)]
pub struct GeminiConfig {
    pub model: String,
    pub auth: GeminiAuth,
    pub settings: GeminiSettings,
    /// `None` = [`GEMINI_DEFAULT_TEMPERATURE`] (a request's own wins).
    pub temperature: Option<f32>,
}

pub struct GeminiProvider {
    http: Client,
    cfg: GeminiConfig,
    /// The ADC bearer token (unused in API-key mode).
    token: Mutex<String>,
    /// Replaces `https://<host>` (tests, proxies).
    base_override: Option<String>,
    retry: RetryPolicy,
}

impl GeminiProvider {
    /// Build the provider. In ADC mode this fetches the first token now, so a
    /// machine without `gcloud` hears about it at start, not mid-chat.
    pub fn new(cfg: GeminiConfig) -> Result<Self, ProviderError> {
        let token = match cfg.auth {
            GeminiAuth::GcloudAdc => {
                if cfg.settings.project.is_none() {
                    return Err(ProviderError::Config("set vein.gemini.project in Settings".into()));
                }
                Self::fetch_token().map_err(ProviderError::Config)?
            }
            GeminiAuth::ApiKey(_) => String::new(),
        };
        Self::with_token(cfg, token)
    }

    /// Build without fetching a token (tests; a caller that already has one).
    pub fn with_token(cfg: GeminiConfig, token: String) -> Result<Self, ProviderError> {
        let http = ClientBuilder::new()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| ProviderError::Config(format!("failed to build HTTP client: {e}")))?;
        info!("Gemini provider: model {} ({:?})", cfg.model, std::mem::discriminant(&cfg.auth));
        Ok(GeminiProvider { http, cfg, token: Mutex::new(token), base_override: None, retry: RetryPolicy::default() })
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_override = Some(url.into().trim_end_matches('/').to_string());
        self
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// The gcloud ADC auth mode's token source.
    pub fn fetch_token() -> Result<String, String> {
        info!("Executing gcloud ADC token fetch...");
        let output = Command::new("gcloud")
            .args(["auth", "application-default", "print-access-token"])
            .output()
            .map_err(|e| {
                error!("gcloud execution failed: {}", e);
                format!("Gemini (gcloud auth): could not run gcloud: {e} — install gcloud, or set vein.gemini.auth = \"api_key\"")
            })?;
        if !output.status.success() {
            let err_msg = String::from_utf8_lossy(&output.stderr);
            error!("gcloud ADC failed: {}", err_msg);
            return Err("Gemini (gcloud auth): no access token — run `gcloud auth application-default login`".to_string());
        }
        String::from_utf8(output.stdout)
            .map(|s| s.trim().to_string())
            .map_err(|_| "Invalid UTF-8 in gcloud token".to_string())
    }

    fn host(&self, region: &str) -> String {
        if let Some(b) = &self.base_override {
            return b.clone();
        }
        match self.cfg.auth {
            GeminiAuth::ApiKey(_) => "https://generativelanguage.googleapis.com".into(),
            GeminiAuth::GcloudAdc if region == "global" => "https://aiplatform.googleapis.com".into(),
            GeminiAuth::GcloudAdc => format!("https://{region}-aiplatform.googleapis.com"),
        }
    }

    /// The URL for `verb` (`generateContent`, `streamGenerateContent?alt=sse`,
    /// `predict`, `embedContent`) on `model`.
    pub fn url(&self, model: &str, verb: &str, embed: bool) -> String {
        let s = &self.cfg.settings;
        let region = if embed { &s.embed_region } else { &s.region };
        let host = self.host(region);
        match self.cfg.auth {
            GeminiAuth::ApiKey(_) => format!("{host}/v1beta/models/{model}:{verb}"),
            GeminiAuth::GcloudAdc => format!(
                "{host}/v1beta1/projects/{}/locations/{region}/publishers/google/models/{model}:{verb}",
                s.project.as_deref().unwrap_or("")
            ),
        }
    }

    /// The `generateContent` body (public for the golden test).
    pub fn build_body(&self, req: &ChatRequest) -> Value {
        let contents: Vec<Content> = req
            .messages
            .iter()
            .map(|m| Content {
                role: match m.role {
                    Role::User => "user".into(),
                    Role::Assistant => "model".into(),
                },
                parts: m.parts.clone(),
            })
            .collect();
        let mut gen_cfg = json!({
            "temperature": f32_json(req.temperature.or(self.cfg.temperature).unwrap_or(GEMINI_DEFAULT_TEMPERATURE))
        });
        if req.max_tokens > 0 {
            gen_cfg["maxOutputTokens"] = json!(req.max_tokens);
        }
        let mut body = json!({ "contents": contents, "generationConfig": gen_cfg });
        if let Some(sys) = req.system.as_ref().filter(|s| !s.trim().is_empty()) {
            body["systemInstruction"] = json!({ "parts": [{ "text": sys }] });
        }
        body
    }

    fn post(&self, url: &str, body: &Value) -> RequestBuilder {
        let rb = self.http.post(url).header("content-type", "application/json");
        let rb = match &self.cfg.auth {
            GeminiAuth::ApiKey(k) => rb.header("x-goog-api-key", k),
            GeminiAuth::GcloudAdc => rb.bearer_auth(self.token.lock().map(|t| t.clone()).unwrap_or_default()),
        };
        rb.body(serde_json::to_vec(body).unwrap_or_default())
    }

    /// Send with the shared backoff; in ADC mode a 401 refreshes the token and
    /// tries once more.
    async fn send(&self, url: &str, body: &Value) -> Result<reqwest::Response, ProviderError> {
        match send_classified(self.post(url, body), &self.retry).await {
            Err(ProviderError::Request { status: 401, .. }) if self.cfg.auth == GeminiAuth::GcloudAdc => {
                warn!("401 Unauthorized detected. Refreshing the gcloud token...");
                let t = Self::fetch_token().map_err(ProviderError::Config)?;
                if let Ok(mut g) = self.token.lock() {
                    *g = t;
                }
                send_classified(self.post(url, body), &self.retry).await
            }
            other => other,
        }
    }
}

// --- response shapes ---

#[derive(Deserialize, Debug, Default)]
pub struct GenerateContentResponse {
    pub candidates: Option<Vec<Candidate>>,
    #[serde(rename = "promptFeedback")]
    pub prompt_feedback: Option<PromptFeedback>,
    #[serde(rename = "usageMetadata")]
    pub usage_metadata: Option<super::UsageMetadata>,
}

#[derive(Deserialize, Debug)]
pub struct Candidate {
    pub content: Option<ContentResponse>,
    #[serde(rename = "finishReason")]
    pub finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct ContentResponse {
    #[serde(default)]
    pub parts: Vec<PartResponse>,
}

#[derive(Deserialize, Debug)]
pub struct PartResponse {
    pub text: Option<String>,
    #[serde(default)]
    pub thought: bool,
}

#[derive(Deserialize, Debug)]
pub struct PromptFeedback {
    #[serde(rename = "blockReason")]
    pub block_reason: Option<String>,
}

/// `finishReason` → [`StopReason`].
pub fn map_finish(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::EndTurn,
        "MAX_TOKENS" => StopReason::MaxTokens,
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY" => {
            StopReason::Refusal { category: Some(reason.to_string()) }
        }
        other => StopReason::Other(other.to_string()),
    }
}

fn usage_of(u: &Option<super::UsageMetadata>) -> Option<Usage> {
    u.as_ref().map(|m| Usage {
        input_tokens: m.prompt_token_count.unwrap_or(0).max(0) as u64,
        output_tokens: m.candidates_token_count.unwrap_or(0).max(0) as u64,
    })
}

/// One response (or one stream chunk): `(text, stop, usage)`. A prompt block
/// is a refusal with the block reason as its category.
pub fn parse_chunk(data: &GenerateContentResponse) -> (String, Option<StopReason>, Option<Usage>) {
    if let Some(reason) = data.prompt_feedback.as_ref().and_then(|f| f.block_reason.clone()) {
        return (String::new(), Some(StopReason::Refusal { category: Some(reason) }), usage_of(&data.usage_metadata));
    }
    let mut text = String::new();
    let mut stop = None;
    if let Some(first) = data.candidates.as_ref().and_then(|c| c.first()) {
        stop = first.finish_reason.as_deref().map(map_finish);
        if let Some(content) = &first.content {
            for p in content.parts.iter().filter(|p| !p.thought) {
                if let Some(t) = &p.text {
                    text.push_str(t);
                }
            }
        }
    }
    (text, stop, usage_of(&data.usage_metadata))
}

pub fn parse_response(data: &GenerateContentResponse) -> Result<ChatResponse, ProviderError> {
    let (text, stop, usage) = parse_chunk(data);
    let stop = match stop {
        Some(s) => s,
        None if text.is_empty() => return Err(ProviderError::Malformed("Gemini returned no candidates".into())),
        None => StopReason::EndTurn,
    };
    Ok(ChatResponse { text, stop, usage: usage.unwrap_or_default() })
}

/// Gemini's SSE: every event is a whole `GenerateContentResponse` chunk; the
/// stream simply ends after the chunk carrying `finishReason`.
#[derive(Debug, Default)]
pub struct GeminiSse {
    usage: Usage,
    stop: Option<StopReason>,
    finished: bool,
}

impl SseHandler for GeminiSse {
    fn on_event(&mut self, ev: &SseEvent) -> Vec<Result<ChatDelta, ProviderError>> {
        if ev.data.trim().is_empty() {
            return vec![];
        }
        let data: GenerateContentResponse = match serde_json::from_str(&ev.data) {
            Ok(d) => d,
            Err(e) => return vec![Err(ProviderError::Malformed(format!("bad Gemini chunk: {e}")))],
        };
        let (text, stop, usage) = parse_chunk(&data);
        if let Some(u) = usage {
            self.usage = u;
        }
        let mut out = Vec::new();
        if !text.is_empty() {
            out.push(Ok(ChatDelta::Text(text)));
        }
        if let Some(StopReason::Refusal { .. }) = &stop {
            self.finished = true;
            out.push(Ok(ChatDelta::Stop { stop: stop.clone().unwrap(), usage: self.usage }));
        } else if stop.is_some() {
            self.stop = stop;
        }
        out
    }

    fn on_end(&mut self) -> Vec<Result<ChatDelta, ProviderError>> {
        self.finished = true;
        match self.stop.take() {
            Some(stop) => vec![Ok(ChatDelta::Stop { stop, usage: self.usage })],
            None => vec![Err(ProviderError::Retryable { status: 0, message: "Gemini stream ended without a finishReason".into() })],
        }
    }

    fn done(&self) -> bool {
        self.finished
    }
}

#[derive(Serialize)]
struct EmbedContentRequest {
    instances: Vec<EmbedContentInstance>,
}

#[derive(Serialize)]
struct EmbedContentInstance {
    content: String,
}

#[derive(Deserialize)]
struct EmbedContentResponse {
    predictions: Option<Vec<EmbedPrediction>>,
    /// The Generative Language API's `embedContent` answer.
    embedding: Option<EmbedValues>,
}

#[derive(Deserialize)]
struct EmbedPrediction {
    embeddings: EmbedValues,
}

#[derive(Deserialize)]
struct EmbedValues {
    values: Vec<f32>,
}

impl ModelProvider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    fn model(&self) -> &str {
        &self.cfg.model
    }

    fn generate<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, Result<ChatResponse, ProviderError>> {
        Box::pin(async move {
            let url = self.url(&self.cfg.model, "generateContent", false);
            let res = self.send(&url, &self.build_body(req)).await?;
            let data: GenerateContentResponse = res.json().await.map_err(|e| ProviderError::Malformed(e.to_string()))?;
            parse_response(&data)
        })
    }

    fn stream<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, Result<DeltaStream<'a>, ProviderError>> {
        Box::pin(async move {
            let url = self.url(&self.cfg.model, "streamGenerateContent?alt=sse", false);
            let res = self.send(&url, &self.build_body(req)).await?;
            Ok(sse_stream(res, GeminiSse::default()))
        })
    }
}

// ---------------------------------------------------------------------------
// EMBED (B317): the embedding call lives here, behind `Embedder` — the chat
// provider no longer embeds.
// ---------------------------------------------------------------------------

impl GeminiProvider {
    /// One embedding of `text` with the configured `embed_model` (Vertex
    /// `predict` under gcloud ADC, `embedContent` under an API key).
    async fn embed_one(&self, text: &str) -> Result<Vec<f32>, ProviderError> {
        let em = &self.cfg.settings.embed_model;
        let (url, body) = match self.cfg.auth {
            GeminiAuth::GcloudAdc => (
                self.url(em, "predict", true),
                serde_json::to_value(EmbedContentRequest {
                    instances: vec![EmbedContentInstance { content: text.to_string() }],
                })
                .unwrap_or_default(),
            ),
            GeminiAuth::ApiKey(_) => (self.url(em, "embedContent", true), json!({ "content": { "parts": [{ "text": text }] } })),
        };
        let res = self.send(&url, &body).await?;
        let data: EmbedContentResponse =
            res.json().await.map_err(|e| ProviderError::Malformed(format!("embedding: {e}")))?;
        if let Some(e) = data.embedding {
            return Ok(e.values);
        }
        data.predictions
            .and_then(|p| p.into_iter().next())
            .map(|p| p.embeddings.values)
            .ok_or_else(|| ProviderError::Malformed("no embedding returned".into()))
    }
}

/// Gemini's embedding model behind [`Embedder`] (`vein.embed.provider = "gemini"`).
pub struct GeminiEmbedder {
    inner: GeminiProvider,
    dims: usize,
}

impl GeminiEmbedder {
    /// Build. In ADC mode this fetches the first token now (a machine without
    /// gcloud hears about it at start). `settings.embed_model` is the model.
    pub fn new(auth: GeminiAuth, settings: GeminiSettings, dims: usize) -> Result<Self, ProviderError> {
        let model = settings.embed_model.clone();
        let inner = GeminiProvider::new(GeminiConfig { model, auth, settings, temperature: None })?;
        Ok(GeminiEmbedder { inner, dims })
    }

    /// Build without fetching a token (tests).
    pub fn with_token(auth: GeminiAuth, settings: GeminiSettings, dims: usize, token: String) -> Result<Self, ProviderError> {
        let model = settings.embed_model.clone();
        let inner = GeminiProvider::with_token(GeminiConfig { model, auth, settings, temperature: None }, token)?;
        Ok(GeminiEmbedder { inner, dims })
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.inner = self.inner.with_base_url(url);
        self
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.inner = self.inner.with_retry(retry);
        self
    }
}

impl Embedder for GeminiEmbedder {
    fn name(&self) -> &str {
        "gemini"
    }

    fn model(&self) -> &str {
        &self.inner.cfg.settings.embed_model
    }

    fn dims(&self) -> usize {
        self.dims
    }

    fn embed<'a>(&'a self, texts: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<Vec<f32>>, ProviderError>> {
        Box::pin(async move {
            let mut out = Vec::with_capacity(texts.len());
            for t in texts {
                out.push(self.inner.embed_one(t).await?);
            }
            check_batch("gemini", self.dims, texts.len(), &out)?;
            Ok(out)
        })
    }
}
