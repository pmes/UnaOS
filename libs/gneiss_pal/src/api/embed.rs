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

//! The embedder seam (EMBED, rmbp-ledger B317; R81).
//!
//! The embedder is its OWN setting, independent of the chat provider: Claude
//! (the default chat provider) has no embeddings endpoint, so semantic recall
//! cannot ride on the chat trait. Every vector Vein stores goes through
//! [`Embedder`]; which one runs comes from Principia's `vein` namespace
//! (`embed.provider`, `embed.model`, `embed.dims`).
//!
//! - [`super::gemini::GeminiEmbedder`] — Gemini's embedding model (Vertex
//!   `predict` with gcloud ADC, or the Generative Language API with a key).
//! - `LocalEmbedder` (feature `local-embed`) — all-MiniLM-L6-v2 in-process, no
//!   network; see [`super::local`].
//! - [`NoEmbedder`] — dims 0: every store writes no vector and recall is OFF,
//!   said in-chat.
//!
//! A vector is only ever compared with vectors from the same model: the vault
//! stores [`Embedder::tag`] (`<provider>/<model>`) as `una:embed-model` beside
//! each vector.

use bandy::PrefValue;

use super::provider::{
    AuthMode, BoxFuture, GEMINI_DEFAULT_EMBED_MODEL, GEMINI_DEFAULT_KEY_ENV, GeminiSettings, ProviderError,
};

/// The local model the `local` embedder runs by default.
pub const LOCAL_DEFAULT_EMBED_MODEL: &str = "all-MiniLM-L6-v2";

/// The in-chat line when no embedder is configured.
pub const RECALL_OFF_NO_EMBEDDER: &str = ":: BRAIN :: RECALL OFF :: no embedder — set vein.embed.provider";

/// Known output widths. A model not listed takes `embed.dims`, or is learned
/// from its first answer (`dims() == 0` with a non-`off` embedder).
pub fn known_dims(model: &str) -> usize {
    match model {
        "text-embedding-004" | "text-embedding-005" | "text-multilingual-embedding-002" => 768,
        "gemini-embedding-001" => 3072,
        "all-MiniLM-L6-v2" | "all-MiniLM-L12-v2" | "paraphrase-MiniLM-L6-v2" => 384,
        _ => 0,
    }
}

/// The seam. Object-safe (`Arc<dyn Embedder>`), async via boxed futures (the
/// Gemini embedder is an HTTP call; the local one computes inline).
pub trait Embedder: Send + Sync {
    /// `"gemini"`, `"local"` or `"off"`.
    fn name(&self) -> &str;

    /// The embedding model (`"none"` for [`NoEmbedder`]).
    fn model(&self) -> &str;

    /// Output width; 0 for [`NoEmbedder`] (and for a model whose width is
    /// learned from its first answer).
    fn dims(&self) -> usize;

    /// One vector per text, in order. [`NoEmbedder`] answers empty vectors.
    fn embed<'a>(&'a self, texts: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<Vec<f32>>, ProviderError>>;

    /// True when this embedder writes vectors (recall is on).
    fn enabled(&self) -> bool {
        true
    }

    /// The tag the vault stores as `una:embed-model`: `<provider>/<model>`.
    fn tag(&self) -> String {
        format!("{}/{}", self.name(), self.model())
    }
}

/// No embedder: recall is off, every store writes no vector.
pub struct NoEmbedder;

impl Embedder for NoEmbedder {
    fn name(&self) -> &str {
        "off"
    }
    fn model(&self) -> &str {
        "none"
    }
    fn dims(&self) -> usize {
        0
    }
    fn enabled(&self) -> bool {
        false
    }
    fn embed<'a>(&'a self, texts: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<Vec<f32>>, ProviderError>> {
        let n = texts.len();
        Box::pin(async move { Ok(vec![Vec::new(); n]) })
    }
}

/// Checks a batch answer: one vector per text, each `dims` wide when `dims`
/// is known. A width mismatch is [`ProviderError::Malformed`] — never stored.
pub fn check_batch(name: &str, dims: usize, want: usize, got: &[Vec<f32>]) -> Result<(), ProviderError> {
    if got.len() != want {
        return Err(ProviderError::Malformed(format!("{name} embedder answered {} vectors for {want} texts", got.len())));
    }
    if dims > 0 {
        if let Some(v) = got.iter().find(|v| v.len() != dims) {
            return Err(ProviderError::Malformed(format!(
                "{name} embedder answered {} dims, configured {dims} (vein.embed.dims)",
                v.len()
            )));
        }
    }
    Ok(())
}

/// Which embedder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedKind {
    Gemini,
    Local,
    Off,
}

impl EmbedKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gemini" | "google" | "vertex" => Some(EmbedKind::Gemini),
            "local" | "minilm" => Some(EmbedKind::Local),
            "off" | "none" | "disabled" => Some(EmbedKind::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            EmbedKind::Gemini => "gemini",
            EmbedKind::Local => "local",
            EmbedKind::Off => "off",
        }
    }
}

/// Everything needed to construct an embedder. Holds the NAME of a key's env
/// var, never the key.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbedConfig {
    pub kind: EmbedKind,
    pub model: String,
    /// 0 = learned from the first answer (or `off`).
    pub dims: usize,
    /// True when `embed.provider` was not set and the default was resolved.
    pub defaulted: bool,
    /// Gemini only.
    pub gemini_auth: AuthMode,
    /// Gemini only: `project`, `embed_region` (and `embed_model == model`).
    pub gemini: GeminiSettings,
}

fn pref_str(v: Option<PrefValue>) -> Option<String> {
    match v {
        Some(PrefValue::Str(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

impl EmbedConfig {
    /// Build from preference lookups in namespace `vein` and an environment
    /// (the environment is only asked whether the Gemini key variable is SET,
    /// to resolve the default; the key itself is read at build time).
    ///
    /// Keys: `embed.provider` (`gemini`|`local`|`off`; default `gemini` when
    /// the `gemini.api_key_env` variable — default `GEMINI_API_KEY` — is set
    /// or `gemini.project` is configured, else `off`), `embed.model` (default
    /// `gemini.embed_model`, then `text-embedding-004`; `all-MiniLM-L6-v2` for
    /// `local`), `embed.dims` (default: the model's known width), and the
    /// Gemini keys it shares with the chat provider: `gemini.auth`,
    /// `gemini.api_key_env`, `gemini.project`, `gemini.embed_region`.
    pub fn from_prefs(
        get: impl Fn(&str) -> Option<PrefValue>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ProviderError> {
        let key_env = pref_str(get("gemini.api_key_env")).unwrap_or_else(|| GEMINI_DEFAULT_KEY_ENV.into());
        let key_set = env(&key_env).is_some_and(|k| !k.trim().is_empty());
        let project = pref_str(get("gemini.project"));
        let (kind, defaulted) = match pref_str(get("embed.provider")) {
            Some(s) => (
                EmbedKind::parse(&s).ok_or_else(|| {
                    ProviderError::Config(format!(
                        "unknown embedder \"{s}\" in vein.embed.provider — choose \"gemini\", \"local\" or \"off\""
                    ))
                })?,
                false,
            ),
            None if key_set || project.is_some() => (EmbedKind::Gemini, true),
            None => (EmbedKind::Off, true),
        };
        let model = match kind {
            EmbedKind::Off => "none".to_string(),
            EmbedKind::Gemini => pref_str(get("embed.model"))
                .or_else(|| pref_str(get("gemini.embed_model")))
                .unwrap_or_else(|| GEMINI_DEFAULT_EMBED_MODEL.into()),
            EmbedKind::Local => pref_str(get("embed.model")).unwrap_or_else(|| LOCAL_DEFAULT_EMBED_MODEL.into()),
        };
        let dims = match (kind, get("embed.dims")) {
            (EmbedKind::Off, _) => 0,
            (_, Some(PrefValue::Int(n))) if n > 0 => n as usize,
            _ => known_dims(&model),
        };
        let gemini_auth = match pref_str(get("gemini.auth")).as_deref() {
            Some("api_key") | Some("apikey") | Some("key") => AuthMode::ApiKeyEnv(key_env),
            Some("gcloud") | Some("adc") | Some("gcloud_adc") => AuthMode::GcloudAdc,
            Some(other) if kind == EmbedKind::Gemini => {
                return Err(ProviderError::Config(format!(
                    "unknown vein.gemini.auth \"{other}\" — use \"gcloud\" or \"api_key\""
                )));
            }
            _ if key_set => AuthMode::ApiKeyEnv(key_env),
            _ => AuthMode::GcloudAdc,
        };
        if kind == EmbedKind::Gemini && gemini_auth == AuthMode::GcloudAdc && project.is_none() {
            return Err(ProviderError::Config(
                "the Gemini embedder via gcloud needs vein.gemini.project (or set GEMINI_API_KEY / vein.gemini.api_key_env)".into(),
            ));
        }
        let mut gemini = GeminiSettings { project, embed_model: model.clone(), ..Default::default() };
        if let Some(r) = pref_str(get("gemini.embed_region")) {
            gemini.embed_region = r;
        }
        Ok(EmbedConfig { kind, model, dims, defaulted, gemini_auth, gemini })
    }

    /// `<provider>/<model>` — the tag a vector from this config carries.
    pub fn tag(&self) -> String {
        format!("{}/{}", self.kind.as_str(), self.model)
    }
}

/// Construct the configured embedder, reading any key from the process
/// environment. Failure is a [`ProviderError::Config`] naming the fix.
pub fn build_embedder(cfg: &EmbedConfig) -> Result<Box<dyn Embedder>, ProviderError> {
    build_embedder_with_env(cfg, |k| std::env::var(k).ok())
}

/// [`build_embedder`] with an injectable environment (tests).
pub fn build_embedder_with_env(
    cfg: &EmbedConfig,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Box<dyn Embedder>, ProviderError> {
    match cfg.kind {
        EmbedKind::Off => Ok(Box::new(NoEmbedder)),
        EmbedKind::Gemini => {
            let auth = match &cfg.gemini_auth {
                AuthMode::ApiKeyEnv(var) => match env(var).map(|k| k.trim().to_string()) {
                    Some(k) if !k.is_empty() => super::gemini::GeminiAuth::ApiKey(k),
                    _ => {
                        return Err(ProviderError::Config(format!(
                            "set {var} for the Gemini embedder, or set vein.embed.provider to \"local\" or \"off\""
                        )));
                    }
                },
                AuthMode::GcloudAdc => super::gemini::GeminiAuth::GcloudAdc,
            };
            Ok(Box::new(super::gemini::GeminiEmbedder::new(auth, cfg.gemini.clone(), cfg.dims)?))
        }
        EmbedKind::Local => build_local(cfg),
    }
}

#[cfg(feature = "local-embed")]
fn build_local(cfg: &EmbedConfig) -> Result<Box<dyn Embedder>, ProviderError> {
    Ok(Box::new(super::local::LocalEmbedder::load(&cfg.model, cfg.dims)?))
}

#[cfg(not(feature = "local-embed"))]
fn build_local(_cfg: &EmbedConfig) -> Result<Box<dyn Embedder>, ProviderError> {
    Err(ProviderError::Config(
        "this build has no local embedder (gneiss_pal feature `local-embed`) — set vein.embed.provider to \"gemini\" or \"off\"".into(),
    ))
}

/// The settings-surface label: `claude / claude-opus-5-5 · embed gemini/text-embedding-004`.
pub fn provider_label(chat: Option<(&str, &str)>, embed: Option<&EmbedConfig>) -> String {
    let chat = match chat {
        Some((p, m)) => format!("{p} / {m}"),
        None => "no provider".to_string(),
    };
    match embed {
        Some(e) if e.kind == EmbedKind::Off => format!("{chat} · embed off"),
        Some(e) => format!("{chat} · embed {}", e.tag()),
        None => format!("{chat} · embed unconfigured"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn prefs(pairs: &[(&str, PrefValue)]) -> impl Fn(&str) -> Option<PrefValue> {
        let m: HashMap<String, PrefValue> = pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        move |k| m.get(k).cloned()
    }
    fn s(v: &str) -> PrefValue {
        PrefValue::Str(v.into())
    }
    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn default_is_off_without_gemini_config() {
        let c = EmbedConfig::from_prefs(prefs(&[]), none).unwrap();
        assert_eq!((c.kind, c.dims, c.defaulted), (EmbedKind::Off, 0, true));
        assert_eq!(c.tag(), "off/none");
        let e = build_embedder_with_env(&c, none).unwrap();
        assert!(!e.enabled());
        assert_eq!(e.dims(), 0);
    }

    #[test]
    fn default_is_gemini_when_the_key_var_is_set() {
        let env = |k: &str| (k == "GEMINI_API_KEY").then(|| "g".to_string());
        let c = EmbedConfig::from_prefs(prefs(&[]), env).unwrap();
        assert_eq!(c.kind, EmbedKind::Gemini);
        assert_eq!(c.model, "text-embedding-004");
        assert_eq!(c.dims, 768);
        assert_eq!(c.gemini_auth, AuthMode::ApiKeyEnv("GEMINI_API_KEY".into()));
        let e = build_embedder_with_env(&c, env).unwrap();
        assert_eq!((e.name(), e.model(), e.dims()), ("gemini", "text-embedding-004", 768));
        assert_eq!(e.tag(), "gemini/text-embedding-004");
    }

    #[test]
    fn default_is_gemini_when_a_project_is_configured() {
        let c = EmbedConfig::from_prefs(prefs(&[("gemini.project", s("p"))]), none).unwrap();
        assert_eq!((c.kind, c.gemini_auth.clone()), (EmbedKind::Gemini, AuthMode::GcloudAdc));
        assert_eq!(c.gemini.project.as_deref(), Some("p"));
    }

    #[test]
    fn embedder_is_independent_of_the_chat_provider() {
        // Chat on Claude, embedder explicitly local: neither reads the other.
        let c = EmbedConfig::from_prefs(
            prefs(&[("provider", s("claude")), ("embed.provider", s("local"))]),
            none,
        )
        .unwrap();
        assert_eq!((c.kind, c.model.as_str(), c.dims), (EmbedKind::Local, "all-MiniLM-L6-v2", 384));
        assert!(!c.defaulted);
    }

    #[test]
    fn explicit_off_and_overrides() {
        let env = |_: &str| Some("k".to_string());
        let c = EmbedConfig::from_prefs(prefs(&[("embed.provider", s("off"))]), env).unwrap();
        assert_eq!(c.kind, EmbedKind::Off);
        let c = EmbedConfig::from_prefs(
            prefs(&[("embed.model", s("some-embedder")), ("embed.dims", PrefValue::Int(256))]),
            env,
        )
        .unwrap();
        assert_eq!((c.model.as_str(), c.dims), ("some-embedder", 256));
        let c = EmbedConfig::from_prefs(prefs(&[("gemini.embed_model", s("text-embedding-005"))]), env).unwrap();
        assert_eq!((c.model.as_str(), c.dims), ("text-embedding-005", 768));
    }

    #[test]
    fn unknown_embedder_and_gcloud_without_project_are_config_errors() {
        let e = EmbedConfig::from_prefs(prefs(&[("embed.provider", s("hal"))]), none).unwrap_err();
        assert!(matches!(e, ProviderError::Config(ref m) if m.contains("vein.embed.provider")));
        let e = EmbedConfig::from_prefs(prefs(&[("embed.provider", s("gemini"))]), none).unwrap_err();
        assert!(matches!(e, ProviderError::Config(ref m) if m.contains("vein.gemini.project")));
    }

    #[test]
    fn missing_key_at_build_is_a_config_error_naming_the_var() {
        let c = EmbedConfig::from_prefs(
            prefs(&[("embed.provider", s("gemini")), ("gemini.auth", s("api_key"))]),
            none,
        )
        .unwrap();
        let e = build_embedder_with_env(&c, none).err().unwrap();
        assert!(matches!(e, ProviderError::Config(ref m) if m.starts_with("set GEMINI_API_KEY")), "{e:?}");
    }

    #[test]
    fn no_embedder_answers_empty_vectors() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let v = rt.block_on(NoEmbedder.embed(&["a", "b"])).unwrap();
        assert_eq!(v, vec![Vec::<f32>::new(), Vec::new()]);
    }

    #[test]
    fn batch_width_is_checked() {
        assert!(check_batch("x", 2, 1, &[vec![1.0, 2.0]]).is_ok());
        assert!(matches!(check_batch("x", 3, 1, &[vec![1.0]]), Err(ProviderError::Malformed(_))));
        assert!(matches!(check_batch("x", 0, 2, &[vec![1.0]]), Err(ProviderError::Malformed(_))));
    }

    #[test]
    fn label_names_both_halves() {
        let c = EmbedConfig::from_prefs(prefs(&[]), |_| Some("k".into())).unwrap();
        assert_eq!(
            provider_label(Some(("claude", "claude-opus-5-5")), Some(&c)),
            "claude / claude-opus-5-5 · embed gemini/text-embedding-004"
        );
        let off = EmbedConfig::from_prefs(prefs(&[]), none).unwrap();
        assert_eq!(provider_label(Some(("claude", "claude-opus-5-5")), Some(&off)), "claude / claude-opus-5-5 · embed off");
    }
}
