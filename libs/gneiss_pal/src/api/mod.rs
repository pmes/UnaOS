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

//! The model client seam (VEINPROV, rmbp-ledger B303).
//!
//! Vein talks to whichever provider the person configured through one trait,
//! [`ModelProvider`]. Today: [`ClaudeProvider`] (Anthropic Messages API, raw
//! HTTP) and [`GeminiProvider`] (Vertex AI with gcloud ADC, or the Generative
//! Language API with a key). Nothing is hardwired: [`ProviderConfig`] comes
//! from Principia's preferences (namespace `vein`) and the key from the env var
//! a preference names. [`Content`]/[`Part`] are Vein's message parts (Gemini
//! wire shape); each provider documents what it does with an attachment.

pub mod claude;
pub mod embed;
pub mod format;
pub mod gemini;
#[cfg(feature = "local-embed")]
pub mod local;
pub mod provider;
pub mod retry;
pub mod sse;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

pub use claude::ClaudeProvider;
pub use embed::{
    EmbedConfig, EmbedKind, Embedder, LOCAL_DEFAULT_EMBED_MODEL, NoEmbedder, RECALL_OFF_NO_EMBEDDER, build_embedder,
    build_embedder_with_env, known_dims, provider_label,
};
pub use gemini::{GeminiAuth, GeminiConfig, GeminiEmbedder, GeminiProvider};
pub use provider::{
    AuthMode, BoxFuture, ChatDelta, ChatMessage, ChatRequest, ChatResponse, DeltaStream, GeminiSettings, MODEL_CHOICES,
    ModelProvider, PREF_NS, model_menu, ProviderConfig, ProviderError, ProviderKind, Role, StopReason, Usage, build_provider,
    build_provider_with_env,
};
pub use retry::RetryPolicy;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Content {
    pub role: String,
    pub parts: Vec<Part>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum Part {
    Text {
        text: String,
    },
    FileData {
        #[serde(rename = "fileData")]
        file_data: FileData,
    },
}

impl Part {
    pub fn text(t: String) -> Self {
        Part::Text { text: t }
    }

    pub fn file_data(mime_type: String, file_uri: String) -> Self {
        Part::FileData {
            file_data: FileData {
                mime_type,
                file_uri,
            },
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileData {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "fileUri")]
    pub file_uri: String,
}

/// Gemini's `usageMetadata` (kept public: the Gemini provider maps it to
/// [`Usage`]).
#[derive(Deserialize, Debug, Clone)]
pub struct UsageMetadata {
    #[serde(rename = "promptTokenCount")]
    pub prompt_token_count: Option<i32>,
    #[serde(rename = "candidatesTokenCount")]
    pub candidates_token_count: Option<i32>,
    #[serde(rename = "totalTokenCount")]
    pub total_token_count: Option<i32>,
}
