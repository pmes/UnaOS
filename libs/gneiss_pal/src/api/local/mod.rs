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

//! The local embedder (EMBED, B317; R81: "a LOCAL embedding model is the arc
//! that makes recall work with no network").
//!
//! all-MiniLM-L6-v2 (BERT, 6 layers, hidden 384, 12 heads), run in-process on
//! the CPU by candle (`candle-transformers::models::bert::BertModel`). Its
//! weights are read straight out of the pinned ONNX export by [`onnx`] (the
//! six per-layer `MatMul` weights are renamed through their bias `Add`);
//! tokenization is [`wordpiece`]. The output is the attention-masked mean of
//! the last hidden state, L2-normalised (the sentence-transformers recipe).
//!
//! The model files are NOT in the repository: `tools/una-models fetch
//! all-MiniLM-L6-v2` puts them in `~/.cache/unaos/models/<name>` (sha256
//! pinned). Their absence is a [`ProviderError::Config`] naming that command —
//! Vein says it in-chat; never a panic.

pub mod onnx;
pub mod wordpiece;

use std::collections::HashMap;
use std::path::PathBuf;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};

use super::embed::{Embedder, check_batch};
use super::provider::{BoxFuture, ProviderError};
use wordpiece::WordPiece;

/// Tokens per text, including `[CLS]`/`[SEP]` (sentence-transformers' cap for this model).
pub const MAX_TOKENS: usize = 256;

/// The files a local model directory holds.
pub const MODEL_FILES: [&str; 3] = ["model.onnx", "vocab.txt", "config.json"];

/// `$XDG_CACHE_HOME/unaos/models/<name>`, else `$HOME/.cache/unaos/models/<name>`.
pub fn model_dir(name: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"));
    base.join("unaos").join("models").join(name)
}

/// The in-chat message when a model is not installed.
pub fn not_installed(name: &str) -> String {
    format!(
        "local embedder: model {name} is not installed in {} — run `tools/una-models fetch {name}`",
        model_dir(name).display()
    )
}

pub struct LocalEmbedder {
    name: String,
    dims: usize,
    bert: BertModel,
    tok: WordPiece,
    device: Device,
}

fn cfg_err(name: &str, what: impl std::fmt::Display) -> ProviderError {
    ProviderError::Config(format!("local embedder {name}: {what}"))
}

impl LocalEmbedder {
    /// Load `name` from [`model_dir`]. `dims` (0 = take the model's) must
    /// match the model's hidden size.
    pub fn load(name: &str, dims: usize) -> Result<Self, ProviderError> {
        Self::load_from(name, &model_dir(name), dims)
    }

    pub fn load_from(name: &str, dir: &std::path::Path, dims: usize) -> Result<Self, ProviderError> {
        if MODEL_FILES.iter().any(|f| !dir.join(f).is_file()) {
            return Err(ProviderError::Config(not_installed(name)));
        }
        let read = |f: &str| std::fs::read(dir.join(f)).map_err(|e| cfg_err(name, format!("{f}: {e}")));
        let config: Config = serde_json::from_slice(&read("config.json")?).map_err(|e| cfg_err(name, format!("config.json: {e}")))?;
        if dims != 0 && dims != config.hidden_size {
            return Err(cfg_err(name, format!("the model is {} wide, vein.embed.dims says {dims}", config.hidden_size)));
        }
        let vocab = String::from_utf8(read("vocab.txt")?).map_err(|_| cfg_err(name, "vocab.txt is not UTF-8"))?;
        let tok = WordPiece::from_vocab(&vocab).map_err(|e| cfg_err(name, e))?;
        let mut graph = onnx::parse_model(&read("model.onnx")?).map_err(|e| cfg_err(name, format!("model.onnx: {e}")))?;
        onnx::name_linear_weights(&mut graph).map_err(|e| cfg_err(name, e))?;
        let device = Device::Cpu;
        let mut tensors = HashMap::new();
        for (k, t) in graph.initializers {
            let tensor = Tensor::from_vec(t.data, t.dims.as_slice(), &device).map_err(|e| cfg_err(name, format!("{k}: {e}")))?;
            tensors.insert(k, tensor);
        }
        let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);
        let bert = BertModel::load(vb, &config).map_err(|e| cfg_err(name, format!("weights: {e}")))?;
        Ok(LocalEmbedder { name: name.to_string(), dims: config.hidden_size, bert, tok, device })
    }

    /// Token ids for `text` (exposed for the golden test).
    pub fn token_ids(&self, text: &str) -> Vec<u32> {
        self.tok.encode(text, MAX_TOKENS)
    }

    /// One normalised sentence vector, synchronously.
    pub fn embed_text(&self, text: &str) -> Result<Vec<f32>, ProviderError> {
        let m = |e: candle_core::Error| ProviderError::Malformed(format!("local embedder: {e}"));
        let ids = self.token_ids(text);
        let n = ids.len();
        let input = Tensor::from_vec(ids, (1, n), &self.device).map_err(m)?;
        let types = Tensor::zeros((1, n), DType::U32, &self.device).map_err(m)?;
        let mask = Tensor::ones((1, n), DType::U32, &self.device).map_err(m)?;
        let hidden = self.bert.forward(&input, &types, Some(&mask)).map_err(m)?; // [1, n, d]
        // One unpadded text: the masked mean is the plain mean over tokens.
        let pooled = hidden.mean(1).map_err(m)?.squeeze(0).map_err(m)?;
        let mut v: Vec<f32> = pooled.to_vec1().map_err(m)?;
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        v.iter_mut().for_each(|x| *x /= norm);
        Ok(v)
    }
}

impl Embedder for LocalEmbedder {
    fn name(&self) -> &str {
        "local"
    }

    fn model(&self) -> &str {
        &self.name
    }

    fn dims(&self) -> usize {
        self.dims
    }

    fn embed<'a>(&'a self, texts: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<Vec<f32>>, ProviderError>> {
        Box::pin(async move {
            let out = texts.iter().map(|t| self.embed_text(t)).collect::<Result<Vec<_>, _>>()?;
            check_batch("local", self.dims, texts.len(), &out)?;
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_model_is_a_config_error_naming_the_command() {
        let dir = std::env::temp_dir().join("unaos-embed-test-no-model-here");
        let e = LocalEmbedder::load_from("all-MiniLM-L6-v2", &dir, 0).err().unwrap();
        assert!(matches!(e, ProviderError::Config(ref m) if m.contains("tools/una-models fetch all-MiniLM-L6-v2")), "{e:?}");
    }
}
