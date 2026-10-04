// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! The provider slot (VEINPROV, rmbp-ledger B303).
//!
//! Vein talks to the provider the person configured, through
//! `gneiss_pal::api::ModelProvider`. The choice is a preference in Principia's
//! store, namespace `vein` (keys: `provider`, `model`, `max_tokens`,
//! `temperature`, `claude.api_key_env`, `claude.fallbacks`, `gemini.project`,
//! `gemini.region`, `gemini.auth`, `gemini.api_key_env`, `gemini.embed_model`,
//! `gemini.embed_region`). The API key is never a preference: the preference
//! names the environment variable it is read from. Holocron takes custody of
//! the credential when it exists.
//!
//! The slot is built at start and rebuilt on `PrincipiaCommand::PrefChanged`
//! for namespace `vein`. A slot that could not be built (no key, no project,
//! unreadable preferences) still answers: every call is an in-chat error
//! naming the fix — never a panic, never a silent fallback to another provider.

use std::path::Path;
use std::sync::Arc;

use bandy::{PrefValue, PrincipiaCommand, SMessage};
use gneiss_pal::api::{
    ChatRequest, ChatResponse, ModelProvider, PREF_NS, Part, ProviderConfig, StopReason, build_provider_with_env,
};
use principia::prefs::PrefStore;

pub struct ProviderSlot {
    inner: Result<(Arc<dyn ModelProvider>, ProviderConfig), String>,
}

impl ProviderSlot {
    /// Build from the standard preference file and the process environment.
    pub fn load() -> Self {
        Self::load_from(&principia::default_prefs_path(), |k| std::env::var(k).ok())
    }

    /// Build from a preference file (a missing file is an empty store: every
    /// default applies) and an environment.
    pub fn load_from(path: &Path, env: impl Fn(&str) -> Option<String>) -> Self {
        match PrefStore::load(path) {
            Ok(store) => Self::from_lookup(|k| store.get(PREF_NS, k), env),
            Err(e) => ProviderSlot {
                inner: Err(format!("preferences unreadable ({}): {e:#}", path.display())),
            },
        }
    }

    /// Build from preference lookups in namespace `vein` and an environment.
    pub fn from_lookup(get: impl Fn(&str) -> Option<PrefValue>, env: impl Fn(&str) -> Option<String>) -> Self {
        let inner = ProviderConfig::from_prefs(get)
            .and_then(|cfg| build_provider_with_env(&cfg, env).map(|p| (Arc::from(p), cfg)))
            .map_err(|e| e.to_string());
        ProviderSlot { inner }
    }

    /// The live provider, or the reason there is none.
    pub fn provider(&self) -> Result<Arc<dyn ModelProvider>, String> {
        self.inner.as_ref().map(|(p, _)| p.clone()).map_err(Clone::clone)
    }

    /// The console line for this slot.
    pub fn status_line(&self) -> String {
        match &self.inner {
            Ok((p, _)) => format!(":: BRAIN :: ONLINE ({} / {})\n\n", p.name(), p.model()),
            Err(e) => format!(":: BRAIN :: NO PROVIDER :: {e}\n\n"),
        }
    }

    /// A one-turn request carrying the configured output cap and temperature.
    pub fn request(&self, system: Option<String>, parts: Vec<Part>) -> ChatRequest {
        let mut req = ChatRequest::single(system, parts);
        if let Ok((_, cfg)) = &self.inner {
            req.max_tokens = cfg.max_tokens;
            req.temperature = cfg.temperature;
        }
        req
    }

    pub async fn generate(&self, req: &ChatRequest) -> Result<ChatResponse, String> {
        let p = self.provider()?;
        p.generate(req).await.map_err(|e| format!("{} :: {e}", p.name()))
    }

    /// An embedding for the vault. A provider without embeddings (Claude)
    /// answers an error; the callers already store an empty vector then.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let p = self.provider()?;
        p.embed(text).await.map_err(|e| e.to_string())
    }
}

/// True for a preference change Vein must rebuild its provider on.
pub fn is_vein_pref_change(msg: &SMessage) -> bool {
    matches!(msg, SMessage::Principia(PrincipiaCommand::PrefChanged { ns, .. }) if ns == PREF_NS)
}

/// The text shown for a response — the stop reason is checked before the text
/// is trusted.
pub fn render_reply(resp: &ChatResponse) -> String {
    match &resp.stop {
        StopReason::EndTurn => resp.text.clone(),
        StopReason::MaxTokens => format!("{}\n[truncated: the response hit max_tokens — raise vein.max_tokens]", resp.text),
        StopReason::Refusal { category } => match category {
            Some(c) => format!("[declined by the provider: {c}]"),
            None => "[declined by the provider]".to_string(),
        },
        StopReason::Other(r) => {
            if resp.text.is_empty() {
                format!("[stopped: {r}]")
            } else {
                format!("{}\n[stopped: {r}]", resp.text)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gneiss_pal::api::Usage;

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn missing_key_is_an_in_chat_error_naming_the_fix() {
        let dir = tempfile::tempdir().unwrap();
        let slot = ProviderSlot::load_from(&dir.path().join("preferences.toml"), none);
        assert_eq!(
            slot.status_line(),
            ":: BRAIN :: NO PROVIDER :: set ANTHROPIC_API_KEY, or choose a provider in Settings\n\n"
        );
        assert!(slot.provider().is_err());
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let e = rt.block_on(slot.generate(&slot.request(None, vec![Part::text("hi".into())]))).unwrap_err();
        assert!(e.contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn reads_the_vein_namespace_from_principias_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.toml");
        let mut store = PrefStore::empty(&path);
        store.set("vein", "provider", PrefValue::Str("gemini".into())).unwrap();
        store.set("vein", "gemini.auth", PrefValue::Str("api_key".into())).unwrap();
        store.set("vein", "gemini.api_key_env", PrefValue::Str("MY_GEMINI".into())).unwrap();
        store.set("vein", "model", PrefValue::Str("gemini-test-model".into())).unwrap();
        let slot = ProviderSlot::load_from(&path, |k| (k == "MY_GEMINI").then(|| "g-key".to_string()));
        assert_eq!(slot.status_line(), ":: BRAIN :: ONLINE (gemini / gemini-test-model)\n\n");
        // The key is never written to the preference file.
        assert!(!std::fs::read_to_string(&path).unwrap().contains("g-key"));

        let slot = ProviderSlot::load_from(&path, none);
        assert_eq!(slot.status_line(), ":: BRAIN :: NO PROVIDER :: set MY_GEMINI, or choose a provider in Settings\n\n");
    }

    #[test]
    fn claude_default_and_request_carries_config() {
        let slot = ProviderSlot::from_lookup(
            |k| match k {
                "max_tokens" => Some(PrefValue::Int(2048)),
                _ => None,
            },
            |k| (k == "ANTHROPIC_API_KEY").then(|| "sk".to_string()),
        );
        assert_eq!(slot.status_line(), ":: BRAIN :: ONLINE (claude / claude-opus-5-5)\n\n");
        assert_eq!(slot.request(None, vec![]).max_tokens, 2048);
    }

    #[test]
    fn pref_change_filter() {
        let mk = |ns: &str| {
            SMessage::Principia(PrincipiaCommand::PrefChanged {
                ns: ns.into(),
                key: "provider".into(),
                value: PrefValue::Str("claude".into()),
            })
        };
        assert!(is_vein_pref_change(&mk("vein")));
        assert!(!is_vein_pref_change(&mk("stria")));
    }

    #[test]
    fn reply_checks_stop_reason_first() {
        let r = |text: &str, stop| ChatResponse { text: text.into(), stop, usage: Usage::default() };
        assert_eq!(render_reply(&r("ok", StopReason::EndTurn)), "ok");
        assert_eq!(
            render_reply(&r("partial", StopReason::Refusal { category: Some("cyber".into()) })),
            "[declined by the provider: cyber]"
        );
        assert!(render_reply(&r("long", StopReason::MaxTokens)).contains("max_tokens"));
    }
}
