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

//! The Claude provider: raw HTTP to the Messages API (VEINPROV, B303).
//!
//! Rust has no official Anthropic SDK; raw HTTP over the crate's `reqwest` is
//! the sanctioned path. `POST {base}/v1/messages` with `x-api-key`,
//! `anthropic-version: 2023-06-01` and a JSON body. Rules this connector keeps:
//!
//! - The default model is `claude-opus-5-5` (exact id, no date suffix).
//! - No `thinking` field is ever sent: adaptive thinking is the model's
//!   default, and a `disabled` type or a `budget_tokens` is a 400.
//! - No assistant prefill: the last message is always the person's.
//! - `stop_reason` is read BEFORE the content; `refusal` becomes
//!   [`StopReason::Refusal`] with `stop_details.category`.
//! - Server-side fallbacks are ON by default: header
//!   `anthropic-beta: server-side-fallback-2026-07-01` and body
//!   `"fallbacks": "default"`. The preference `vein.claude.fallbacks = false`
//!   turns both off.
//! - 400/401/403/404 are final ([`ProviderError::Request`]); 408/409/429/5xx and
//!   connection errors are retried through [`super::retry`] (a 429's
//!   `retry-after` is honoured).
//!
//! Attachments: Vein's [`Part::FileData`] is Gemini-shaped (`fileUri`). Claude
//! gets an `https://` PDF as a `document` block and an `https://` image as an
//! `image` block, both with a `url` source; anything else (a `gs://` upload, a
//! non-PDF document) is a [`ProviderError::Unsupported`] refusal to attach —
//! never a silent drop.

use reqwest::{Client, ClientBuilder, RequestBuilder};
use serde_json::{Value, json};
use std::time::Duration;

use super::Part;
use super::provider::{
    BoxFuture, ChatDelta, ChatRequest, ChatResponse, DEFAULT_MAX_TOKENS, DeltaStream, ModelProvider, ProviderError,
    Role, StopReason, Usage,
};
use super::retry::{RetryPolicy, send_classified};
use super::sse::{SseEvent, SseHandler, sse_stream};

pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

pub struct ClaudeProvider {
    http: Client,
    key: String,
    model: String,
    base_url: String,
    fallbacks: bool,
    retry: RetryPolicy,
}

impl ClaudeProvider {
    pub fn new(key: String, model: String, fallbacks: bool) -> Result<Self, ProviderError> {
        let http = ClientBuilder::new()
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|e| ProviderError::Config(format!("failed to build HTTP client: {e}")))?;
        Ok(ClaudeProvider { http, key, model, base_url: ANTHROPIC_BASE_URL.into(), fallbacks, retry: RetryPolicy::default() })
    }

    /// Point at another host (a test server, a proxy).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into().trim_end_matches('/').to_string();
        self
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// The request body for `req` (public so the shape can be pinned by a
    /// golden test).
    pub fn build_body(&self, req: &ChatRequest, stream: bool) -> Result<Value, ProviderError> {
        let mut messages = Vec::with_capacity(req.messages.len());
        for m in &req.messages {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            let mut content = Vec::with_capacity(m.parts.len());
            for p in &m.parts {
                if let Some(block) = part_to_block(p)? {
                    content.push(block);
                }
            }
            if content.is_empty() {
                return Err(ProviderError::Unsupported("an empty message (Claude needs non-blank text)".into()));
            }
            messages.push(json!({ "role": role, "content": content }));
        }
        if req.messages.last().map(|m| m.role) != Some(Role::User) {
            return Err(ProviderError::Unsupported("a conversation that does not end with the person's turn (no prefill)".into()));
        }
        let max_tokens = if req.max_tokens == 0 { DEFAULT_MAX_TOKENS } else { req.max_tokens };
        let mut body = json!({ "model": self.model, "max_tokens": max_tokens, "messages": messages });
        let obj = body.as_object_mut().expect("object literal");
        if let Some(sys) = req.system.as_ref().filter(|s| !s.trim().is_empty()) {
            obj.insert("system".into(), json!(sys));
        }
        if let Some(t) = req.temperature {
            obj.insert("temperature".into(), super::provider::f32_json(t));
        }
        if stream {
            obj.insert("stream".into(), json!(true));
        }
        if self.fallbacks {
            obj.insert("fallbacks".into(), json!("default"));
        }
        Ok(body)
    }

    fn post(&self, body: &Value) -> RequestBuilder {
        let mut rb = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json");
        if self.fallbacks {
            rb = rb.header("anthropic-beta", FALLBACK_BETA);
        }
        rb.body(serde_json::to_vec(body).unwrap_or_default())
    }
}

/// One Vein part → one Claude content block (`None` = blank text, skipped:
/// the API rejects whitespace-only text blocks).
fn part_to_block(p: &Part) -> Result<Option<Value>, ProviderError> {
    match p {
        Part::Text { text } => {
            if text.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(json!({ "type": "text", "text": text })))
            }
        }
        Part::FileData { file_data } => {
            let uri = file_data.file_uri.as_str();
            let mime = file_data.mime_type.as_str();
            if uri.starts_with("https://") {
                if mime == "application/pdf" {
                    return Ok(Some(json!({ "type": "document", "source": { "type": "url", "url": uri } })));
                }
                if matches!(mime, "image/jpeg" | "image/png" | "image/gif" | "image/webp") {
                    return Ok(Some(json!({ "type": "image", "source": { "type": "url", "url": uri } })));
                }
            }
            Err(ProviderError::Unsupported(format!(
                "attaching {mime} from {uri} — Claude reads https:// PDF and image URLs only; this upload is Gemini-only (choose Gemini in Settings to send it)"
            )))
        }
    }
}

/// `stop_reason` (+ `stop_details`) → [`StopReason`].
pub fn map_stop(reason: Option<&str>, details: Option<&Value>) -> StopReason {
    match reason {
        Some("end_turn") | Some("stop_sequence") => StopReason::EndTurn,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("refusal") => StopReason::Refusal {
            category: details.and_then(|d| d.get("category")).and_then(|c| c.as_str()).map(str::to_string),
        },
        Some(other) => StopReason::Other(other.to_string()),
        None => StopReason::Other("none".into()),
    }
}

fn usage_of(v: Option<&Value>) -> (Option<u64>, Option<u64>) {
    let get = |k: &str| v.and_then(|u| u.get(k)).and_then(|n| n.as_u64());
    (get("input_tokens"), get("output_tokens"))
}

/// Parse a non-streaming Messages response. `stop_reason` first, then the
/// `text` blocks in order (thinking and any other block types are skipped).
pub fn parse_message(v: &Value) -> Result<ChatResponse, ProviderError> {
    if v.get("type").and_then(|t| t.as_str()) == Some("error") {
        return Err(stream_error(v));
    }
    let stop = map_stop(v.get("stop_reason").and_then(|s| s.as_str()), v.get("stop_details"));
    let blocks = v
        .get("content")
        .and_then(|c| c.as_array())
        .ok_or_else(|| ProviderError::Malformed("no content array".into()))?;
    let mut text = String::new();
    for b in blocks {
        if b.get("type").and_then(|t| t.as_str()) == Some("text") {
            text.push_str(b.get("text").and_then(|t| t.as_str()).unwrap_or(""));
        }
    }
    let (i, o) = usage_of(v.get("usage"));
    Ok(ChatResponse { text, stop, usage: Usage { input_tokens: i.unwrap_or(0), output_tokens: o.unwrap_or(0) } })
}

/// An in-stream (or body) `{"type":"error","error":{...}}` → a classified error.
fn stream_error(v: &Value) -> ProviderError {
    let kind = v.pointer("/error/type").and_then(|t| t.as_str()).unwrap_or("");
    let message = v.pointer("/error/message").and_then(|t| t.as_str()).unwrap_or(kind).to_string();
    match kind {
        "overloaded_error" => ProviderError::Retryable { status: 529, message },
        "api_error" => ProviderError::Retryable { status: 500, message },
        "rate_limit_error" => ProviderError::Retryable { status: 429, message },
        _ => ProviderError::Request { status: 400, message },
    }
}

/// The SSE translation: `message_start` (input usage), `content_block_delta`
/// with `text_delta` (text), `message_delta` (stop reason, output usage),
/// `message_stop` (the terminal [`ChatDelta::Stop`]); `ping`,
/// `content_block_start/stop` and non-text deltas are skipped.
#[derive(Debug, Default)]
pub struct ClaudeSse {
    usage: Usage,
    stop: Option<StopReason>,
    finished: bool,
}

impl SseHandler for ClaudeSse {
    fn on_event(&mut self, ev: &SseEvent) -> Vec<Result<ChatDelta, ProviderError>> {
        let v: Value = match serde_json::from_str(&ev.data) {
            Ok(v) => v,
            Err(_) if ev.data.trim().is_empty() => return vec![],
            Err(e) => return vec![Err(ProviderError::Malformed(format!("bad SSE data: {e}")))],
        };
        let kind = v.get("type").and_then(|t| t.as_str()).or(ev.event.as_deref()).unwrap_or("");
        match kind {
            "message_start" => {
                let (i, o) = usage_of(v.pointer("/message/usage"));
                self.usage.input_tokens = i.unwrap_or(0);
                self.usage.output_tokens = o.unwrap_or(0);
                vec![]
            }
            "content_block_delta" => {
                if v.pointer("/delta/type").and_then(|t| t.as_str()) == Some("text_delta") {
                    let t = v.pointer("/delta/text").and_then(|t| t.as_str()).unwrap_or("");
                    if !t.is_empty() {
                        return vec![Ok(ChatDelta::Text(t.to_string()))];
                    }
                }
                vec![]
            }
            "message_delta" => {
                if let Some(r) = v.pointer("/delta/stop_reason").and_then(|s| s.as_str()) {
                    let details = v.pointer("/delta/stop_details").or_else(|| v.get("stop_details"));
                    self.stop = Some(map_stop(Some(r), details));
                }
                let (i, o) = usage_of(v.get("usage"));
                if let Some(i) = i {
                    self.usage.input_tokens = i;
                }
                if let Some(o) = o {
                    self.usage.output_tokens = o;
                }
                vec![]
            }
            "message_stop" => {
                self.finished = true;
                let stop = self.stop.take().unwrap_or(StopReason::Other("none".into()));
                vec![Ok(ChatDelta::Stop { stop, usage: self.usage })]
            }
            "error" => {
                self.finished = true;
                vec![Err(stream_error(&v))]
            }
            _ => vec![],
        }
    }

    fn on_end(&mut self) -> Vec<Result<ChatDelta, ProviderError>> {
        vec![Err(ProviderError::Retryable { status: 0, message: "stream ended before message_stop".into() })]
    }

    fn done(&self) -> bool {
        self.finished
    }
}

impl ModelProvider for ClaudeProvider {
    fn name(&self) -> &str {
        "claude"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn generate<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, Result<ChatResponse, ProviderError>> {
        Box::pin(async move {
            let body = self.build_body(req, false)?;
            let res = send_classified(self.post(&body), &self.retry).await?;
            let v: Value = res.json().await.map_err(|e| ProviderError::Malformed(e.to_string()))?;
            parse_message(&v)
        })
    }

    fn stream<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, Result<DeltaStream<'a>, ProviderError>> {
        Box::pin(async move {
            let body = self.build_body(req, true)?;
            let res = send_classified(self.post(&body), &self.retry).await?;
            Ok(sse_stream(res, ClaudeSse::default()))
        })
    }
}
