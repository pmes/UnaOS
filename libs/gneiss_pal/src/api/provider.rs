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

//! The provider seam (VEINPROV, rmbp-ledger B303; ROADMAP SH-4).
//!
//! Vein owns the provider abstraction (CODEX §2: Vein = AI, "Provider
//! Abstraction (Local/Cloud)"). Every model call goes through
//! [`ModelProvider`]; the concrete providers ([`super::claude::ClaudeProvider`],
//! [`super::gemini::GeminiProvider`]) translate the provider-neutral
//! [`ChatRequest`] into their own wire shape. No provider is hardwired: which
//! one runs, its model and its auth mode come from a [`ProviderConfig`] that
//! Vein reads from Principia's preference store (namespace `vein`).
//!
//! The API key itself is never a preference: the preference NAMES an
//! environment variable and the key is read from it. Holocron takes custody of
//! the credential when it exists (it is design-only today).

use std::future::Future;
use std::pin::Pin;

use bandy::PrefValue;
use futures_core::Stream;

use super::Part;

/// A boxed, sendable future — the object-safe shape of an `async fn` in a
/// trait that is used as `dyn ModelProvider`.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A boxed stream of incremental response pieces.
pub type DeltaStream<'a> = Pin<Box<dyn Stream<Item = Result<ChatDelta, ProviderError>> + Send + 'a>>;

/// Who authored a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// One turn of a conversation. `parts` reuses Vein's [`Part`]: text, or a
/// Gemini-shaped `file_data` attachment (see each provider for what it does
/// with an attachment — never a silent drop).
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub parts: Vec<Part>,
}

impl ChatMessage {
    pub fn user_text(text: impl Into<String>) -> Self {
        ChatMessage { role: Role::User, parts: vec![Part::text(text.into())] }
    }
}

/// A provider-neutral request.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// Operator framing (the system prompt). `None` sends none.
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    /// Upper bound on output tokens.
    pub max_tokens: u32,
    /// `None` leaves the provider's own default in place.
    pub temperature: Option<f32>,
}

/// An `f32` as the JSON number a person typed (`0.4`, not `0.4000000059604645`).
pub fn f32_json(v: f32) -> serde_json::Value {
    v.to_string().parse::<f64>().map(serde_json::Value::from).unwrap_or(serde_json::Value::Null)
}

/// The default output cap: 16000 tokens.
pub const DEFAULT_MAX_TOKENS: u32 = 16000;

impl ChatRequest {
    /// A single user turn with an optional system prompt.
    pub fn single(system: Option<String>, parts: Vec<Part>) -> Self {
        ChatRequest {
            system,
            messages: vec![ChatMessage { role: Role::User, parts }],
            max_tokens: DEFAULT_MAX_TOKENS,
            temperature: None,
        }
    }
}

/// Why the model stopped. Callers check this BEFORE reading the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    /// The provider declined. `category` is what the provider said, if anything.
    Refusal { category: Option<String> },
    /// Any other provider-reported reason, verbatim.
    Other(String),
}

/// Token accounting for one call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A complete response.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatResponse {
    pub text: String,
    pub stop: StopReason,
    pub usage: Usage,
}

/// One piece of a streamed response.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatDelta {
    /// A chunk of response text, in order.
    Text(String),
    /// The final piece: why the model stopped, and the usage.
    Stop { stop: StopReason, usage: Usage },
}

/// Classified provider errors — retry and the in-chat message decide on the
/// variant, never by parsing prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// No usable configuration: no key in the named env var, no Gemini
    /// project, an unknown provider name. The message is what the person
    /// should do about it.
    Config(String),
    /// The provider answered a non-retryable status (400/401/403/404 and any
    /// other 4xx not listed as retryable).
    Request { status: u16, message: String },
    /// A retryable failure that exhausted the retry budget (408/409/429/5xx,
    /// or a connection error, `status == 0`).
    Retryable { status: u16, message: String },
    /// The provider answered something this connector could not translate.
    Malformed(String),
    /// The operation (or attachment) is not supported by this provider.
    Unsupported(String),
    /// A local client process failed (CLAUDECODE, SR38: the Claude Code CLI
    /// exited non-zero, or reported an error that is not an API status).
    /// `code` is the exit code (`None`: killed by a signal, or no exit yet).
    Cli { code: Option<i32>, message: String },
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::Config(m) => write!(f, "{m}"),
            ProviderError::Request { status, message } => write!(f, "request rejected ({status}): {message}"),
            ProviderError::Retryable { status: 0, message } => write!(f, "connection failed: {message}"),
            ProviderError::Retryable { status, message } => write!(f, "provider unavailable ({status}): {message}"),
            ProviderError::Malformed(m) => write!(f, "malformed provider response: {m}"),
            ProviderError::Unsupported(m) => write!(f, "not supported by this provider: {m}"),
            ProviderError::Cli { code: Some(c), message } => write!(f, "Claude Code CLI exited with status {c}: {message}"),
            ProviderError::Cli { code: None, message } => write!(f, "Claude Code CLI failed: {message}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// The seam. Object-safe (`Arc<dyn ModelProvider>`), async via boxed futures.
pub trait ModelProvider: Send + Sync {
    /// The provider's stable name (`"claude"`, `"gemini"`, `"claudecode"`). Never a credential.
    fn name(&self) -> &str;

    /// The model this provider was configured with.
    fn model(&self) -> &str;

    /// One request, one complete response.
    fn generate<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, Result<ChatResponse, ProviderError>>;

    /// The same request, answered incrementally: zero or more
    /// [`ChatDelta::Text`] then one [`ChatDelta::Stop`].
    fn stream<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, Result<DeltaStream<'a>, ProviderError>>;

    // EMBED (B317): no `embed` here — the embedder is its own seam (`super::embed::Embedder`).
}

// ---------------------------------------------------------------------------
// Configuration (read from Principia's preference store, namespace `vein`)
// ---------------------------------------------------------------------------

/// The preference namespace Vein reads.
pub const PREF_NS: &str = "vein";

/// Default model per provider. `claude-opus-5-5` is the exact id (no date suffix).
pub const CLAUDE_DEFAULT_MODEL: &str = "claude-opus-5-5";
pub const GEMINI_DEFAULT_MODEL: &str = "gemini-3.1-pro-preview";
pub const GEMINI_DEFAULT_EMBED_MODEL: &str = "text-embedding-004";
/// Default env var names for the API keys (the preference can name another).
pub const CLAUDE_DEFAULT_KEY_ENV: &str = "ANTHROPIC_API_KEY";
pub const GEMINI_DEFAULT_KEY_ENV: &str = "GEMINI_API_KEY";

/// The `(provider, model)` pairs a settings surface offers. A person may type
/// any other model id into the `vein.model` preference; this is only a menu.
pub const MODEL_CHOICES: &[(&str, &str)] = &[
    ("claude", CLAUDE_DEFAULT_MODEL),
    ("gemini", GEMINI_DEFAULT_MODEL),
    ("claudecode", super::claudecode::CLAUDECODE_DEFAULT_MODEL),
];

/// The settings-surface menu: one label per [`MODEL_CHOICES`] entry
/// (`"<model> (<provider>)"`), plus the configured model if a person typed one
/// the menu does not list, and the index to preselect (the configured one, or
/// 0 when nothing is configured).
pub fn model_menu(configured: Option<&ProviderConfig>) -> (Vec<String>, u32) {
    let mut labels: Vec<String> = MODEL_CHOICES.iter().map(|(p, m)| format!("{m} ({p})")).collect();
    let Some(cfg) = configured else { return (labels, 0) };
    let want = format!("{} ({})", cfg.model, cfg.kind.as_str());
    let idx = match labels.iter().position(|l| *l == want) {
        Some(i) => i,
        None => {
            labels.push(want);
            labels.len() - 1
        }
    };
    (labels, idx as u32)
}

/// Which provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Claude,
    Gemini,
    /// CLAUDECODE (SR38): the installed Claude Code CLI, logged into a
    /// subscription — no API key.
    ClaudeCode,
}

impl ProviderKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" | "anthropic" => Some(ProviderKind::Claude),
            "gemini" | "google" | "vertex" => Some(ProviderKind::Gemini),
            "claudecode" | "claude-code" | "claude_code" => Some(ProviderKind::ClaudeCode),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude",
            ProviderKind::Gemini => "gemini",
            ProviderKind::ClaudeCode => "claudecode",
        }
    }
}

/// How the provider authenticates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMode {
    /// An API key read from the environment variable this names.
    ApiKeyEnv(String),
    /// Google application-default credentials (`gcloud auth
    /// application-default print-access-token`). Gemini only.
    GcloudAdc,
    /// The Claude Code CLI holds its own login; Vein never sees a credential.
    Cli,
}

/// Gemini-only settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiSettings {
    /// Vertex project id. Required in [`AuthMode::GcloudAdc`]; unused by the
    /// API-key mode (which talks to the Generative Language API).
    pub project: Option<String>,
    /// Vertex location for generation (`global` → `aiplatform.googleapis.com`).
    pub region: String,
    /// Vertex location for embeddings.
    pub embed_region: String,
    pub embed_model: String,
}

impl Default for GeminiSettings {
    fn default() -> Self {
        GeminiSettings {
            project: None,
            region: "global".into(),
            embed_region: "us-central1".into(),
            embed_model: GEMINI_DEFAULT_EMBED_MODEL.into(),
        }
    }
}

/// Everything needed to construct a provider. Contains the NAME of the key's
/// env var, never the key.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub model: String,
    pub auth: AuthMode,
    /// Default output cap for requests Vein builds.
    pub max_tokens: u32,
    /// `None` = the provider's default.
    pub temperature: Option<f32>,
    /// Claude only: opt into server-side fallbacks (`vein.claude.fallbacks`,
    /// default on).
    pub claude_fallbacks: bool,
    pub gemini: GeminiSettings,
    /// Claude Code only: the CLI binary (`vein.claudecode.bin`, default
    /// `claude` looked up on `PATH`).
    pub claudecode_bin: String,
}

fn pref_str(v: Option<PrefValue>) -> Option<String> {
    match v {
        Some(PrefValue::Str(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

impl ProviderConfig {
    /// Build the config from preference lookups in namespace `vein`. `get`
    /// answers one key (`"provider"`, `"claude.api_key_env"`, ...). Defaults
    /// live here, with the consumer, never in the store.
    ///
    /// Keys: `provider` (`claude`|`gemini`|`claudecode`, default `claude`), `model`,
    /// `max_tokens`, `temperature`, `claude.api_key_env` (default
    /// `ANTHROPIC_API_KEY`), `claude.fallbacks` (default true),
    /// `gemini.project`, `gemini.region` (default `global`), `gemini.auth`
    /// (`gcloud`|`api_key`, default `gcloud`), `gemini.api_key_env` (default
    /// `GEMINI_API_KEY`), `gemini.embed_model`, `gemini.embed_region`,
    /// `claudecode.bin` (default `claude` on `PATH`).
    pub fn from_prefs(get: impl Fn(&str) -> Option<PrefValue>) -> Result<Self, ProviderError> {
        let kind = match pref_str(get("provider")) {
            None => ProviderKind::Claude,
            Some(s) => ProviderKind::parse(&s).ok_or_else(|| {
                ProviderError::Config(format!(
                    "unknown provider \"{s}\" in vein.provider — choose \"claude\", \"gemini\" or \"claudecode\" in Settings"
                ))
            })?,
        };
        let model = pref_str(get("model")).unwrap_or_else(|| match kind {
            ProviderKind::Claude => CLAUDE_DEFAULT_MODEL.into(),
            ProviderKind::Gemini => GEMINI_DEFAULT_MODEL.into(),
            ProviderKind::ClaudeCode => super::claudecode::CLAUDECODE_DEFAULT_MODEL.into(),
        });
        let claudecode_bin =
            pref_str(get("claudecode.bin")).unwrap_or_else(|| super::claudecode::CLAUDECODE_DEFAULT_BIN.into());
        let max_tokens = match get("max_tokens") {
            Some(PrefValue::Int(n)) if n > 0 => n.min(u32::MAX as i64) as u32,
            _ => DEFAULT_MAX_TOKENS,
        };
        let temperature = match get("temperature") {
            Some(PrefValue::Float(f)) => Some(f as f32),
            Some(PrefValue::Int(i)) => Some(i as f32),
            _ => None,
        };
        let claude_fallbacks = !matches!(get("claude.fallbacks"), Some(PrefValue::Bool(false)));
        let mut gemini = GeminiSettings { project: pref_str(get("gemini.project")), ..Default::default() };
        if let Some(r) = pref_str(get("gemini.region")) {
            gemini.region = r;
        }
        if let Some(r) = pref_str(get("gemini.embed_region")) {
            gemini.embed_region = r;
        }
        if let Some(m) = pref_str(get("gemini.embed_model")) {
            gemini.embed_model = m;
        }
        let auth = match kind {
            ProviderKind::Claude => AuthMode::ApiKeyEnv(
                pref_str(get("claude.api_key_env")).unwrap_or_else(|| CLAUDE_DEFAULT_KEY_ENV.into()),
            ),
            ProviderKind::Gemini => match pref_str(get("gemini.auth")).as_deref() {
                None | Some("gcloud") | Some("adc") | Some("gcloud_adc") => AuthMode::GcloudAdc,
                Some("api_key") | Some("apikey") | Some("key") => AuthMode::ApiKeyEnv(
                    pref_str(get("gemini.api_key_env")).unwrap_or_else(|| GEMINI_DEFAULT_KEY_ENV.into()),
                ),
                Some(other) => {
                    return Err(ProviderError::Config(format!(
                        "unknown vein.gemini.auth \"{other}\" — use \"gcloud\" or \"api_key\""
                    )));
                }
            },
            ProviderKind::ClaudeCode => AuthMode::Cli,
        };
        if kind == ProviderKind::Gemini && auth == AuthMode::GcloudAdc && gemini.project.is_none() {
            return Err(ProviderError::Config(
                "Gemini via gcloud needs a Vertex project: set vein.gemini.project in Settings (or vein.gemini.auth = \"api_key\")".into(),
            ));
        }
        Ok(ProviderConfig { kind, model, auth, max_tokens, temperature, claude_fallbacks, gemini, claudecode_bin })
    }

    /// The env var this config reads its key from, if it uses one.
    pub fn key_env(&self) -> Option<&str> {
        match &self.auth {
            AuthMode::ApiKeyEnv(v) => Some(v),
            AuthMode::GcloudAdc | AuthMode::Cli => None,
        }
    }
}

/// Construct the configured provider, reading the API key from the process
/// environment. A missing key is a [`ProviderError::Config`] naming the
/// variable — never a panic, never a fallback to another provider.
pub fn build_provider(cfg: &ProviderConfig) -> Result<Box<dyn ModelProvider>, ProviderError> {
    build_provider_with_env(cfg, |k| std::env::var(k).ok())
}

/// [`build_provider`] with an injectable environment (tests).
pub fn build_provider_with_env(
    cfg: &ProviderConfig,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Box<dyn ModelProvider>, ProviderError> {
    let key = match &cfg.auth {
        AuthMode::ApiKeyEnv(var) => match env(var).map(|k| k.trim().to_string()) {
            Some(k) if !k.is_empty() => Some(k),
            _ => {
                return Err(ProviderError::Config(format!(
                    "set {var}, or choose a provider in Settings"
                )));
            }
        },
        AuthMode::GcloudAdc | AuthMode::Cli => None,
    };
    match cfg.kind {
        ProviderKind::Claude => {
            let key = key.ok_or_else(|| ProviderError::Config("Claude needs an API key env var (vein.claude.api_key_env)".into()))?;
            Ok(Box::new(super::claude::ClaudeProvider::new(key, cfg.model.clone(), cfg.claude_fallbacks)?))
        }
        ProviderKind::Gemini => {
            let auth = match key {
                Some(k) => super::gemini::GeminiAuth::ApiKey(k),
                None => super::gemini::GeminiAuth::GcloudAdc,
            };
            Ok(Box::new(super::gemini::GeminiProvider::new(super::gemini::GeminiConfig {
                model: cfg.model.clone(),
                auth,
                settings: cfg.gemini.clone(),
                temperature: cfg.temperature,
            })?))
        }
        // The CLI is spawned per request: a missing binary or a logged-out
        // CLI is reported on the first message, verbatim, not here.
        ProviderKind::ClaudeCode => {
            Ok(Box::new(super::claudecode::ClaudeCodeProvider::new(cfg.claudecode_bin.clone(), cfg.model.clone())))
        }
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

    #[test]
    fn defaults_are_claude_with_anthropic_key_env() {
        let c = ProviderConfig::from_prefs(prefs(&[])).unwrap();
        assert_eq!(c.kind, ProviderKind::Claude);
        assert_eq!(c.model, "claude-opus-5-5");
        assert_eq!(c.auth, AuthMode::ApiKeyEnv("ANTHROPIC_API_KEY".into()));
        assert_eq!(c.max_tokens, 16000);
        assert!(c.claude_fallbacks);
        assert_eq!(c.temperature, None);
    }

    #[test]
    fn gemini_needs_a_project_under_gcloud() {
        let e = ProviderConfig::from_prefs(prefs(&[("provider", PrefValue::Str("gemini".into()))])).unwrap_err();
        assert!(matches!(e, ProviderError::Config(ref m) if m.contains("vein.gemini.project")), "{e:?}");
        let c = ProviderConfig::from_prefs(prefs(&[
            ("provider", PrefValue::Str("gemini".into())),
            ("gemini.project", PrefValue::Str("my-proj".into())),
            ("gemini.region", PrefValue::Str("europe-west4".into())),
        ]))
        .unwrap();
        assert_eq!(c.auth, AuthMode::GcloudAdc);
        assert_eq!(c.model, GEMINI_DEFAULT_MODEL);
        assert_eq!(c.gemini.project.as_deref(), Some("my-proj"));
        assert_eq!(c.gemini.region, "europe-west4");
    }

    #[test]
    fn gemini_api_key_mode_needs_no_project() {
        let c = ProviderConfig::from_prefs(prefs(&[
            ("provider", PrefValue::Str("gemini".into())),
            ("gemini.auth", PrefValue::Str("api_key".into())),
        ]))
        .unwrap();
        assert_eq!(c.auth, AuthMode::ApiKeyEnv("GEMINI_API_KEY".into()));
    }

    #[test]
    fn unknown_provider_is_a_config_error() {
        let e = ProviderConfig::from_prefs(prefs(&[("provider", PrefValue::Str("hal9000".into()))])).unwrap_err();
        assert!(matches!(e, ProviderError::Config(_)));
    }

    #[test]
    fn overrides_are_read() {
        let c = ProviderConfig::from_prefs(prefs(&[
            ("model", PrefValue::Str("a-configured-model".into())),
            ("claude.api_key_env", PrefValue::Str("MY_KEY".into())),
            ("claude.fallbacks", PrefValue::Bool(false)),
            ("max_tokens", PrefValue::Int(4096)),
            ("temperature", PrefValue::Float(0.25)),
        ]))
        .unwrap();
        assert_eq!(c.model, "a-configured-model");
        assert_eq!(c.key_env(), Some("MY_KEY"));
        assert!(!c.claude_fallbacks);
        assert_eq!(c.max_tokens, 4096);
        assert_eq!(c.temperature, Some(0.25));
    }

    #[test]
    fn model_menu_preselects_the_configured_model() {
        let (labels, i) = model_menu(None);
        assert_eq!(labels[0], "claude-opus-5-5 (claude)");
        assert_eq!(i, 0);
        let mut c = ProviderConfig::from_prefs(prefs(&[
            ("provider", PrefValue::Str("gemini".into())),
            ("gemini.auth", PrefValue::Str("api_key".into())),
        ]))
        .unwrap();
        let (labels, i) = model_menu(Some(&c));
        assert_eq!(labels[i as usize], "gemini-3.1-pro-preview (gemini)");
        c.model = "a-typed-model".into();
        let (labels, i) = model_menu(Some(&c));
        assert_eq!(labels.len(), MODEL_CHOICES.len() + 1);
        assert_eq!(labels[i as usize], "a-typed-model (gemini)");
    }

    #[test]
    fn claudecode_needs_no_key_and_reads_its_binary() {
        let c = ProviderConfig::from_prefs(prefs(&[("provider", PrefValue::Str("claudecode".into()))])).unwrap();
        assert_eq!((c.kind, c.auth.clone(), c.model.as_str(), c.claudecode_bin.as_str()), (ProviderKind::ClaudeCode, AuthMode::Cli, "default", "claude"));
        assert_eq!(c.key_env(), None);
        let p = build_provider_with_env(&c, |_| None).unwrap();
        assert_eq!((p.name(), p.model()), ("claudecode", "default"));
        let c = ProviderConfig::from_prefs(prefs(&[
            ("provider", PrefValue::Str("claude-code".into())),
            ("claudecode.bin", PrefValue::Str("/opt/node22/bin/claude".into())),
            ("model", PrefValue::Str("opus".into())),
        ]))
        .unwrap();
        assert_eq!((c.claudecode_bin.as_str(), c.model.as_str()), ("/opt/node22/bin/claude", "opus"));
        let (labels, i) = model_menu(Some(&c));
        assert_eq!(labels[i as usize], "opus (claudecode)");
        assert!(labels.contains(&"default (claudecode)".to_string()));
    }

    #[test]
    fn missing_key_is_a_clear_error_not_a_fallback() {
        let c = ProviderConfig::from_prefs(prefs(&[])).unwrap();
        let e = build_provider_with_env(&c, |_| None).err().unwrap();
        assert_eq!(e, ProviderError::Config("set ANTHROPIC_API_KEY, or choose a provider in Settings".into()));
        let p = build_provider_with_env(&c, |k| (k == "ANTHROPIC_API_KEY").then(|| "sk-test".to_string())).unwrap();
        assert_eq!(p.name(), "claude");
        assert_eq!(p.model(), "claude-opus-5-5");
    }
}
