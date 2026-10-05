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
//! the CPU by UnaOS's own inference core, `infer_core` (INFERCORE, SR57: the
//! safetensors / ONNX readers, the HF-exact BERT tokenizer and the BERT encoder,
//! `no_std`, zero dependencies). The output is the attention-masked mean of the
//! last hidden state, L2-normalised (the sentence-transformers recipe).
//!
//! Files read from the model directory: `config.json`; the weights from
//! `model.safetensors` (F32 / F16 / BF16) when present, else `model.onnx`; the
//! tokenizer from `tokenizer.json` when present, else `vocab.txt` with the BERT
//! uncased defaults. A batch is split across the CPU's threads — sequences are
//! encoded independently, so the vectors are the same bits whatever the split.
//!
//! The model files are NOT in the repository: `tools/una-models fetch
//! all-MiniLM-L6-v2` puts them in `~/.cache/unaos/models/<name>` (sha256
//! pinned). Their absence is a [`ProviderError::Config`] naming that command —
//! Vein says it in-chat; never a panic.

use std::path::{Path, PathBuf};

use infer_core::bert::{Bert, Config, graph_from_onnx};
use infer_core::safetensors::SafeTensors;
use infer_core::tokenizer::Tokenizer;

use super::embed::{Embedder, check_batch};
use super::provider::{BoxFuture, ProviderError};

/// Tokens per text, including `[CLS]`/`[SEP]` (sentence-transformers' cap for this model).
pub const MAX_TOKENS: usize = 256;

/// The files `tools/una-models` installs for a local model (the embedder also reads
/// `tokenizer.json` and `model.safetensors` when they are there).
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
    bert: Bert,
    tok: Tokenizer,
    threads: usize,
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

    pub fn load_from(name: &str, dir: &Path, dims: usize) -> Result<Self, ProviderError> {
        let has = |f: &str| dir.join(f).is_file();
        if !has("config.json") || !(has("model.safetensors") || has("model.onnx")) || !(has("tokenizer.json") || has("vocab.txt")) {
            return Err(ProviderError::Config(not_installed(name)));
        }
        let read = |f: &str| std::fs::read(dir.join(f)).map_err(|e| cfg_err(name, format!("{f}: {e}")));
        let text = |f: &str| String::from_utf8(read(f)?).map_err(|_| cfg_err(name, format!("{f} is not UTF-8")));
        let config = Config::from_json(&text("config.json")?).map_err(|e| cfg_err(name, e))?;
        if dims != 0 && dims != config.hidden_size {
            return Err(cfg_err(name, format!("the model is {} wide, vein.embed.dims says {dims}", config.hidden_size)));
        }
        let tok = if has("tokenizer.json") {
            Tokenizer::from_tokenizer_json(&text("tokenizer.json")?)
        } else {
            Tokenizer::from_vocab_txt(&text("vocab.txt")?)
        }
        .map_err(|e| cfg_err(name, e))?;
        if tok.id_bound() as usize > config.vocab_size {
            return Err(cfg_err(name, "the tokenizer emits ids past the model's vocabulary"));
        }
        let bert = if has("model.safetensors") {
            let bytes = read("model.safetensors")?;
            let st = SafeTensors::parse(&bytes).map_err(|e| cfg_err(name, format!("model.safetensors: {e}")))?;
            Bert::load(config, &st, false)
        } else {
            let graph = graph_from_onnx(&read("model.onnx")?).map_err(|e| cfg_err(name, format!("model.onnx: {e}")))?;
            Bert::load(config, &graph, false)
        }
        .map_err(|e| cfg_err(name, e))?;
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        Ok(LocalEmbedder { name: name.to_string(), dims: bert.config.hidden_size, bert, tok, threads })
    }

    /// Token ids for `text` (exposed for the golden test).
    pub fn token_ids(&self, text: &str) -> Vec<u32> {
        self.tok.encode_truncated(text, MAX_TOKENS)
    }

    /// One normalised sentence vector, synchronously.
    pub fn embed_text(&self, text: &str) -> Result<Vec<f32>, ProviderError> {
        Ok(self.embed_texts(&[text])?.remove(0))
    }

    /// Normalised sentence vectors for a batch, synchronously, across the CPU's threads.
    pub fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, ProviderError> {
        let ids: Vec<Vec<u32>> = texts.iter().map(|t| self.token_ids(t)).collect();
        let run = |chunk: &[Vec<u32>]| {
            let seqs: Vec<&[u32]> = chunk.iter().map(Vec::as_slice).collect();
            self.bert.embed(&seqs).map_err(|e| ProviderError::Malformed(format!("local embedder: {e}")))
        };
        let threads = self.threads.min(ids.len()).max(1);
        if threads == 1 {
            return run(&ids);
        }
        let per = ids.len().div_ceil(threads);
        let parts: Vec<Result<Vec<Vec<f32>>, ProviderError>> = std::thread::scope(|s| {
            let handles: Vec<_> = ids.chunks(per).map(|c| s.spawn(move || run(c))).collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|_| Err(ProviderError::Malformed("local embedder: a worker thread panicked".into()))))
                .collect()
        });
        let mut out = Vec::with_capacity(ids.len());
        for p in parts {
            out.extend(p?);
        }
        Ok(out)
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
            let out = self.embed_texts(texts)?;
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
