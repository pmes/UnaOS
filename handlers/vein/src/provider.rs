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
//! the credential: [`ProviderSlot::load`] asks Holocron for `vein/claude.api_key`
//! first and reads the env var only when Holocron says NotFound or is absent
//! (HOLOCRON1, LEDGER SR33; `holocron put vein claude.api_key`).
//!
//! The slot is built at start and rebuilt on `PrincipiaCommand::PrefChanged`
//! for namespace `vein`. A slot that could not be built (no key, no project,
//! unreadable preferences) still answers: every call is an in-chat error
//! naming the fix — never a panic, never a silent fallback to another provider.

use std::path::Path;
use std::sync::Arc;

use bandy::{PrefValue, PrincipiaCommand, SMessage};
use gneiss_pal::api::{
    AuthMode, ChatRequest, ChatResponse, EmbedConfig, Embedder, ModelProvider, PREF_NS, Part, ProviderConfig, RECALL_OFF_NO_EMBEDDER,
    ProviderKind, StopReason, Thread, build_embedder_with_env, build_provider_with_env, provider_status,
};
use holocron::holocron_core::keysource::KeySource;
use principia::prefs::PrefStore;

/// The live chat provider, its config, and a SECOND instance of the same
/// provider for side calls (engram compression): a session-keeping provider
/// (the Claude Code CLI) must not have its conversation session replaced by a
/// one-shot side request (VEINTURNS, SR42).
type Live = (Arc<dyn ModelProvider>, ProviderConfig, Arc<dyn ModelProvider>);

pub struct ProviderSlot {
    inner: Result<Live, String>,
}

impl ProviderSlot {
    /// Build from the standard preference file and the process environment, asking Holocron for the
    /// Claude API key FIRST (HOLOCRON1, LEDGER SR33): `SecretGet("vein", "claude.api_key")` on the
    /// Holocron socket; the env var the preference names is the fallback only when Holocron answers
    /// NotFound or is not running. A locked or refusing Holocron is an in-chat error naming the fix —
    /// never a silent fall back to a stale copy in the environment.
    pub fn load() -> Self {
        let sock = holocron::client::default_socket();
        Self::load_with(&principia::default_prefs_path(), |k| std::env::var(k).ok(), || {
            holocron::client::claude_api_key(sock.as_deref())
        })
    }

    /// Build from a preference file (a missing file is an empty store: every
    /// default applies) and an environment. No Holocron is asked.
    pub fn load_from(path: &Path, env: impl Fn(&str) -> Option<String>) -> Self {
        Self::load_with(path, env, || KeySource::Fallback)
    }

    /// [`Self::load_from`] with Holocron's answer for the Claude key (`ask` runs only when the
    /// configured provider is Claude with an API key).
    pub fn load_with(path: &Path, env: impl Fn(&str) -> Option<String>, ask: impl FnOnce() -> KeySource) -> Self {
        match PrefStore::load(path) {
            Ok(store) => Self::from_lookup_with(|k| store.get(PREF_NS, k), env, ask),
            Err(e) => ProviderSlot {
                inner: Err(format!("preferences unreadable ({}): {e:#}", path.display())),
            },
        }
    }

    /// Build from preference lookups in namespace `vein` and an environment. No Holocron is asked.
    pub fn from_lookup(get: impl Fn(&str) -> Option<PrefValue>, env: impl Fn(&str) -> Option<String>) -> Self {
        Self::from_lookup_with(get, env, || KeySource::Fallback)
    }

    /// [`Self::from_lookup`] with the consumer rule of `holocron_core::keysource` applied to the Claude
    /// key: Holocron's bytes win; `Fallback` reads the env var; `Refuse` is the slot's error.
    pub fn from_lookup_with(
        get: impl Fn(&str) -> Option<PrefValue>,
        env: impl Fn(&str) -> Option<String>,
        ask: impl FnOnce() -> KeySource,
    ) -> Self {
        let inner = ProviderConfig::from_prefs(get).map_err(|e| e.to_string()).and_then(|cfg| {
            let held = match (&cfg.kind, &cfg.auth) {
                (ProviderKind::Claude, AuthMode::ApiKeyEnv(var)) => match ask() {
                    KeySource::Holocron(k) => match std::str::from_utf8(k.expose()) {
                        Ok(key) => Some((var.clone(), key.trim().to_string())),
                        Err(_) => return Err("Holocron's vein/claude.api_key is not UTF-8".to_string()),
                    },
                    KeySource::Fallback => None,
                    KeySource::Refuse(why) => return Err(why.to_string()),
                },
                _ => None,
            };
            let env = |k: &str| match &held {
                Some((var, key)) if var == k => Some(key.clone()),
                _ => env(k),
            };
            // VEINTURNS (SR42): two instances — the chat provider keeps the CLI session, engram side calls ride their own.
            let chat = build_provider_with_env(&cfg, &env).map_err(|e| e.to_string())?;
            let side = build_provider_with_env(&cfg, &env).map_err(|e| e.to_string())?;
            Ok((Arc::from(chat), cfg, Arc::from(side)))
        });
        ProviderSlot { inner }
    }

    /// The live provider, or the reason there is none.
    pub fn provider(&self) -> Result<Arc<dyn ModelProvider>, String> {
        self.inner.as_ref().map(|(p, _, _)| p.clone()).map_err(Clone::clone)
    }

    /// The side-call provider (engram compression): the same configuration,
    /// its own instance, so it never touches the conversation's session.
    pub fn side_provider(&self) -> Result<Arc<dyn ModelProvider>, String> {
        self.inner.as_ref().map(|(_, _, s)| s.clone()).map_err(Clone::clone)
    }

    /// The console line for this slot.
    pub fn status_line(&self) -> String {
        format!(":: BRAIN :: {}\n\n", self.status())
    }

    /// `ONLINE (<name> / <model>)` or `NO PROVIDER :: <why>` — the line the
    /// Settings surface shows too ([`provider_status`]).
    pub fn status(&self) -> String {
        match &self.inner {
            Ok((p, _, _)) => provider_status(Ok((p.name(), p.model()))),
            Err(e) => provider_status(Err(e)),
        }
    }

    /// VEINTURNS (SR42): the request for a new user turn in `thread` — the
    /// system prompt alone in `system`, every earlier turn its own message,
    /// then `parts`, with the configured output cap and temperature. (It
    /// replaces the one-turn request with the history folded into `system`.)
    pub fn turn(&self, system: Option<String>, thread: &Thread, parts: Vec<Part>) -> ChatRequest {
        let mut req = thread.request(system, parts);
        if let Ok((_, cfg, _)) = &self.inner {
            req.max_tokens = cfg.max_tokens;
            req.temperature = cfg.temperature;
        }
        req
    }

    /// VEINTURNS (SR42): the status-row text after a turn —
    /// `session=resumed` / `session=new` for a session-keeping provider,
    /// `session=stateless` for the HTTP APIs (each request carries the whole
    /// thread), then `turns=<completed exchanges>`.
    pub fn session_status(&self, thread: &Thread) -> String {
        let word = match self.provider().ok().map(|p| p.session()) {
            Some(Some(s)) => s.label(),
            Some(None) if thread.is_empty() => "session=new",
            Some(None) if self.is_session_keeping() => "session=new",
            Some(None) => "session=stateless",
            None => "session=none",
        };
        format!("{word} · turns={}", thread.exchanges())
    }

    fn is_session_keeping(&self) -> bool {
        matches!(&self.inner, Ok((_, cfg, _)) if cfg.kind == gneiss_pal::api::ProviderKind::ClaudeCode)
    }

    /// VEINTURNS (SR42): `/new` — the provider forgets its kept session.
    pub fn reset_session(&self) {
        if let Ok((p, _, _)) = &self.inner {
            p.reset_session();
        }
    }

    pub async fn generate(&self, req: &ChatRequest) -> Result<ChatResponse, String> {
        let p = self.provider()?;
        p.generate(req).await.map_err(|e| format!("{} :: {e}", p.name()))
    }

    /// The chat provider's `(name, model)`, for the settings label.
    pub fn chat_pair(&self) -> Option<(String, String)> {
        self.inner.as_ref().ok().map(|(p, _, _)| (p.name().to_string(), p.model().to_string()))
    }
}

/// EMBED (B317): the embedder slot — its own setting, independent of the chat
/// provider (R81). Built at start and rebuilt on a `vein` preference change.
#[derive(Clone)]
pub struct EmbedSlot {
    /// What the preferences asked for (`None` = they could not be read/parsed).
    cfg: Option<EmbedConfig>,
    /// The live embedder, or why there is none.
    inner: Result<Arc<dyn Embedder>, String>,
}

/// One text's vector and the tag it carries (`una:embed-model`). An empty
/// vector (recall off, or the embedder failed) is stored as no vector.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Embedded {
    pub vector: Vec<f32>,
    pub tag: String,
}

impl EmbedSlot {
    pub fn load() -> Self {
        Self::load_from(&principia::default_prefs_path(), |k| std::env::var(k).ok())
    }

    pub fn load_from(path: &Path, env: impl Fn(&str) -> Option<String>) -> Self {
        match PrefStore::load(path) {
            Ok(store) => Self::from_lookup(|k| store.get(PREF_NS, k), env),
            Err(e) => EmbedSlot { cfg: None, inner: Err(format!("preferences unreadable ({}): {e:#}", path.display())) },
        }
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<PrefValue>, env: impl Fn(&str) -> Option<String>) -> Self {
        match EmbedConfig::from_prefs(get, &env) {
            Ok(cfg) => {
                let inner = build_embedder_with_env(&cfg, &env).map(Arc::from).map_err(|e| e.to_string());
                EmbedSlot { cfg: Some(cfg), inner }
            }
            Err(e) => EmbedSlot { cfg: None, inner: Err(e.to_string()) },
        }
    }

    /// An embedder directly (tests, and a caller that built one itself).
    pub fn from_embedder(e: Arc<dyn Embedder>) -> Self {
        EmbedSlot { cfg: None, inner: Ok(e) }
    }

    pub fn config(&self) -> Option<&EmbedConfig> {
        self.cfg.as_ref()
    }

    /// True when vectors are written and recall runs.
    pub fn enabled(&self) -> bool {
        matches!(&self.inner, Ok(e) if e.enabled())
    }

    /// The tag a vector from this slot carries; empty when recall is off.
    pub fn tag(&self) -> String {
        match &self.inner {
            Ok(e) if e.enabled() => e.tag(),
            _ => String::new(),
        }
    }

    /// The in-chat reason recall is off, or `None` when it is on.
    pub fn recall_off(&self) -> Option<String> {
        match &self.inner {
            Ok(e) if e.enabled() => None,
            Ok(_) => Some(RECALL_OFF_NO_EMBEDDER.to_string()),
            Err(why) => Some(format!(":: BRAIN :: RECALL OFF :: {why}")),
        }
    }

    /// The console lines at start: `:: BRAIN :: EMBED <provider>/<model> dims=<n>`,
    /// then the recall-off line when recall is off.
    pub fn status_line(&self) -> String {
        let head = match (&self.inner, &self.cfg) {
            (Ok(e), _) => format!(":: BRAIN :: EMBED {}/{} dims={}", e.name(), e.model(), e.dims()),
            (Err(_), Some(c)) => format!(":: BRAIN :: EMBED {} UNAVAILABLE", c.tag()),
            (Err(_), None) => ":: BRAIN :: EMBED UNCONFIGURED".to_string(),
        };
        match self.recall_off() {
            None => format!("{head}\n\n"),
            Some(off) => format!("{head}\n{off}\n\n"),
        }
    }

    /// Embed a batch. Recall off answers empty vectors (stored as no vector).
    pub async fn embed_many(&self, texts: &[&str]) -> Result<Vec<Embedded>, String> {
        let e = match &self.inner {
            Ok(e) if e.enabled() => e.clone(),
            _ => return Ok(vec![Embedded::default(); texts.len()]),
        };
        let tag = e.tag();
        let vecs = e.embed(texts).await.map_err(|err| format!("{} embedder :: {err}", e.name()))?;
        Ok(vecs.into_iter().map(|vector| Embedded { vector, tag: tag.clone() }).collect())
    }

    /// Embed one text for the vault. Failure or recall-off is an empty vector
    /// and the reason (the caller says it in-chat); a store is never blocked.
    pub async fn embed_one(&self, text: &str) -> (Embedded, Option<String>) {
        match self.embed_many(&[text]).await {
            Ok(mut v) => (v.pop().unwrap_or_default(), None),
            Err(why) => (Embedded::default(), Some(format!(":: BRAIN :: EMBED FAILED :: {why}"))),
        }
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
        let e = rt.block_on(slot.generate(&slot.turn(None, &Thread::new(), vec![Part::text("hi".into())]))).unwrap_err();
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

    /// CLAUDECODE (SR38): `vein.provider = "claudecode"` in Principia's file
    /// selects the Claude Code CLI provider — no key needed — and a missing
    /// binary reaches the chat verbatim, naming `vein.claudecode.bin`.
    #[test]
    fn claudecode_is_selected_by_the_pref_and_needs_no_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.toml");
        let mut store = PrefStore::empty(&path);
        store.set("vein", "provider", PrefValue::Str("claudecode".into())).unwrap();
        let bin = dir.path().join("no-such-claude");
        store.set("vein", "claudecode.bin", PrefValue::Str(bin.display().to_string())).unwrap();
        let slot = ProviderSlot::load_from(&path, none);
        assert_eq!(slot.status_line(), ":: BRAIN :: ONLINE (claudecode / default)\n\n");
        assert_eq!(slot.chat_pair(), Some(("claudecode".into(), "default".into())));
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let e = rt.block_on(slot.generate(&slot.turn(None, &Thread::new(), vec![Part::text("hi".into())]))).unwrap_err();
        assert_eq!(
            e,
            format!(
                "claudecode :: Claude Code CLI not found: `{}` — install Claude Code, or set vein.claudecode.bin to the claude binary's path",
                bin.display()
            )
        );
    }

    /// VEINTURNS (SR42): Vein's own path — `ProviderSlot::turn` over a `Thread`,
    /// the provider chosen by Principia's file — rides one Claude Code CLI
    /// session: turn 2 is `--resume <id>` with only the new turn on stdin, the
    /// status row reads `session=resumed`; `/new` resets to `session=new`.
    #[test]
    fn vein_turns_ride_one_cli_session_and_new_starts_fresh() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../libs/gneiss_pal/tests/fixtures/claudecode");
        let d = dir.path().display();
        let script = format!(
            "#!/bin/sh\nD='{d}'\nn=$(cat \"$D/n\" 2>/dev/null || echo 0); n=$((n+1)); echo $n > \"$D/n\"\n\
             for a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$D/argv.$n\"\ncat > \"$D/stdin.$n\"\n\
             case \" $* \" in *\" --resume \"*) cat '{}';; *) cat '{}';; esac\n",
            fx.join("turn2.jsonl").display(),
            fx.join("turn1.jsonl").display()
        );
        let bin = dir.path().join("claude");
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.path().join("preferences.toml");
        let mut store = PrefStore::empty(&path);
        store.set("vein", "provider", PrefValue::Str("claudecode".into())).unwrap();
        store.set("vein", "claudecode.bin", PrefValue::Str(bin.display().to_string())).unwrap();
        let slot = ProviderSlot::load_from(&path, none);
        let read = |f: &str| std::fs::read_to_string(dir.path().join(f)).unwrap_or_default();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

        let mut thread = Thread::new();
        assert_eq!(slot.session_status(&thread), "session=new · turns=0");
        let mut statuses = Vec::new();
        for q in ["Count from one to five.", "Continue to eight."] {
            let parts = vec![Part::text(q.into())];
            let req = slot.turn(Some("sys".into()), &thread, parts.clone());
            assert_eq!(req.system.as_deref(), Some("sys"), "the system prompt alone");
            assert_eq!(req.messages.len(), 2 * thread.exchanges() + 1);
            let reply = rt.block_on(slot.generate(&req)).unwrap();
            thread.record(parts, &reply.text);
            statuses.push(slot.session_status(&thread));
        }
        assert_eq!(statuses, ["session=new · turns=1", "session=resumed · turns=2"]);
        let argv2: Vec<String> = read("argv.2").lines().map(str::to_string).collect();
        assert!(argv2.windows(2).any(|w| w == ["--resume", "00000000-0000-4000-8000-00000000cc01"]), "{argv2:?}");
        assert_eq!(read("stdin.2"), "Continue to eight.", "only the new turn");

        // The side instance (engram compression) never displaces the session.
        let side = slot.side_provider().unwrap();
        rt.block_on(side.generate(&Thread::new().request(None, vec![Part::text("compress".into())]))).unwrap();
        let parts = vec![Part::text("Continue to ten.".into())];
        let reply = rt.block_on(slot.generate(&slot.turn(None, &thread, parts.clone()))).unwrap();
        thread.record(parts, &reply.text);
        assert!(read("argv.4").lines().any(|a| a == "--resume"), "still resumed after a side call");
        assert_eq!(slot.session_status(&thread), "session=resumed · turns=3");

        // `/new`.
        thread.clear();
        slot.reset_session();
        assert_eq!(slot.session_status(&thread), "session=new · turns=0");
        let parts = vec![Part::text("hello".into())];
        let reply = rt.block_on(slot.generate(&slot.turn(None, &thread, parts.clone()))).unwrap();
        thread.record(parts, &reply.text);
        assert!(!read("argv.5").lines().any(|a| a == "--resume"));
        assert_eq!(slot.session_status(&thread), "session=new · turns=1");
    }

    /// VEINTURNS (SR42): the HTTP providers are stateless — the whole thread is
    /// the request — and the status row says so.
    #[test]
    fn api_providers_send_the_thread_and_read_stateless() {
        let slot = ProviderSlot::from_lookup(|_| None, |k| (k == "ANTHROPIC_API_KEY").then(|| "sk".to_string()));
        let mut thread = Thread::new();
        thread.record(vec![Part::text("a".into())], "A");
        thread.record(vec![Part::text("b".into())], "B");
        let req = slot.turn(Some("sys".into()), &thread, vec![Part::text("c".into())]);
        assert_eq!(req.messages.len(), 5);
        assert_eq!(slot.session_status(&thread), "session=stateless · turns=2");
    }

    /// VEINTURNS (SR42) M3: the Settings dropdown's writes — `vein.provider` and
    /// `vein.model` as PrefSets — are accepted by Principia (schema-validated,
    /// persisted, PrefChanged), and the slot Vein rebuilds from the file reads
    /// `ONLINE (claudecode / default)`, the same line the dropdown shows.
    #[test]
    fn settings_dropdown_writes_go_through_principia() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = principia::Principia::with_config_dir(dir.path());
        let writes = gneiss_pal::api::menu_choice_prefs("default (claudecode)").unwrap();
        for (key, value) in writes.iter().cloned() {
            let reply = p.process_impulse(&SMessage::Principia(PrincipiaCommand::PrefSet {
                ns: "vein".into(),
                key: key.into(),
                value: value.clone(),
            }));
            let reply = reply.expect("a set is answered");
            assert!(is_vein_pref_change(&reply), "{reply:?}");
        }
        let slot = ProviderSlot::load_from(&dir.path().join("preferences.toml"), none);
        assert_eq!(slot.status(), "ONLINE (claudecode / default)");
        let get = |k: &str| p.prefs().get("vein", k);
        assert_eq!(gneiss_pal::api::probe_provider(get, none), slot.status());
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
        assert_eq!(slot.turn(None, &Thread::new(), vec![]).max_tokens, 2048);
    }

    // ---- HOLOCRON1 M4: Holocron first, env on NotFound only ----

    fn sk_env(k: &str) -> Option<String> {
        (k == "ANTHROPIC_API_KEY").then(|| "sk-env".to_string())
    }

    #[test]
    fn holocron_key_wins_over_env_and_fills_an_empty_env() {
        use holocron::holocron_core::zero::SecretBytes;
        let slot = ProviderSlot::from_lookup_with(|_| None, none, || KeySource::Holocron(SecretBytes::new(b"sk-ring".to_vec())));
        assert_eq!(slot.status_line(), ":: BRAIN :: ONLINE (claude / claude-opus-5-5)\n\n");
        let slot = ProviderSlot::from_lookup_with(|_| None, sk_env, || KeySource::Holocron(SecretBytes::new(b"sk-ring".to_vec())));
        assert!(slot.provider().is_ok());
    }

    #[test]
    fn holocron_fallback_reads_env_and_refusal_names_the_fix() {
        let slot = ProviderSlot::from_lookup_with(|_| None, sk_env, || KeySource::Fallback);
        assert_eq!(slot.status_line(), ":: BRAIN :: ONLINE (claude / claude-opus-5-5)\n\n");
        // Locked: the env copy is NOT used.
        let slot = ProviderSlot::from_lookup_with(|_| None, sk_env, || KeySource::Refuse("Holocron is locked: run `holocron unlock`"));
        assert_eq!(slot.status_line(), ":: BRAIN :: NO PROVIDER :: Holocron is locked: run `holocron unlock`\n\n");
        // Holocron is not asked for a provider that takes no Claude key.
        let gemini = |k: &str| match k {
            "provider" => Some(PrefValue::Str("gemini".into())),
            "gemini.auth" => Some(PrefValue::Str("api_key".into())),
            _ => None,
        };
        let slot = ProviderSlot::from_lookup_with(gemini, |k| (k == "GEMINI_API_KEY").then(|| "g".into()), || panic!("asked Holocron for gemini"));
        assert!(slot.provider().is_ok());
    }

    #[test]
    fn holocron_daemon_end_to_end() {
        use holocron::daemon::{self, OsEntropy, Shared};
        use holocron::holocron_core::{seal::KdfParams, service::Holocron, testseal::{TestSealer, TestSigner}, wire::{Request, status}};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".holocron");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let me = holocron::principal::my_principal().unwrap();
        let svc = Holocron::new(TestSealer, TestSigner, holocron::store::DirStore::new(&root), OsEntropy, me, KdfParams::DEFAULT);
        let bus = root.join(daemon::BUS_SOCK);
        let l = daemon::bind_private(&bus).unwrap();
        let sh = Shared::new(svc, None);
        std::thread::spawn(move || daemon::accept_loop(l, sh, daemon::serve_bus_conn));
        let ask = || holocron::client::claude_api_key(Some(&bus));
        // Empty Holocron: NotFound -> env.
        assert!(ProviderSlot::from_lookup_with(|_| None, sk_env, ask).provider().is_ok());
        assert!(ProviderSlot::from_lookup_with(|_| None, none, ask).status_line().contains("set ANTHROPIC_API_KEY"));
        // `holocron put vein claude.api_key`, then Vein comes online with no env at all.
        let mut c = holocron::client::Client::connect(&bus).unwrap();
        assert_eq!(c.call(&Request::Unlock { create: true, password: b"pw".to_vec() }).unwrap().status, status::OK);
        let put = Request::Put { ns: "vein".into(), name: "claude.api_key".into(), kind: "api-key".into(), label: "Claude".into(), data: b"sk-ring".to_vec() };
        assert_eq!(c.call(&put).unwrap().status, status::OK);
        assert_eq!(ProviderSlot::from_lookup_with(|_| None, none, ask).status_line(), ":: BRAIN :: ONLINE (claude / claude-opus-5-5)\n\n");
        // Locked: refused even though the env has a key.
        assert_eq!(c.call(&Request::Lock).unwrap().status, status::OK);
        assert!(ProviderSlot::from_lookup_with(|_| None, sk_env, ask).status_line().contains("holocron unlock"));
    }

    #[test]
    fn pref_change_filter() {
        let mk = |ns: &str| {
            SMessage::Principia(PrincipiaCommand::PrefChanged {
                ns: ns.into(),
                key: "provider".into(),
                value: PrefValue::Str("claude".into()),
                clamped: false,
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

    // EMBED (B317): the embedder slot is its own setting.
    #[test]
    fn embed_slot_defaults_off_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let slot = EmbedSlot::load_from(&dir.path().join("preferences.toml"), none);
        assert_eq!(
            slot.status_line(),
            ":: BRAIN :: EMBED off/none dims=0\n:: BRAIN :: RECALL OFF :: no embedder — set vein.embed.provider\n\n"
        );
        assert!(!slot.enabled());
        assert_eq!(slot.tag(), "");
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (e, why) = rt.block_on(slot.embed_one("hello"));
        assert_eq!((e, why), (Embedded::default(), None));
    }

    #[test]
    fn embed_slot_is_gemini_with_a_key_even_when_chat_is_claude() {
        let env = |k: &str| match k {
            "GEMINI_API_KEY" => Some("g".to_string()),
            "ANTHROPIC_API_KEY" => Some("a".to_string()),
            _ => None,
        };
        let chat = ProviderSlot::from_lookup(|_| None, env);
        assert_eq!(chat.status_line(), ":: BRAIN :: ONLINE (claude / claude-opus-5-5)\n\n");
        let slot = EmbedSlot::from_lookup(|_| None, env);
        assert_eq!(slot.status_line(), ":: BRAIN :: EMBED gemini/text-embedding-004 dims=768\n\n");
        assert_eq!(slot.tag(), "gemini/text-embedding-004");
        assert!(slot.recall_off().is_none());
    }

    #[test]
    fn embed_slot_unavailable_names_the_fix() {
        let slot = EmbedSlot::from_lookup(
            |k| match k {
                "embed.provider" => Some(PrefValue::Str("gemini".into())),
                "gemini.auth" => Some(PrefValue::Str("api_key".into())),
                _ => None,
            },
            none,
        );
        let line = slot.status_line();
        assert!(line.starts_with(":: BRAIN :: EMBED gemini/text-embedding-004 UNAVAILABLE\n:: BRAIN :: RECALL OFF :: set GEMINI_API_KEY"), "{line}");
    }

    /// EMBED (B317) M2: `embed.provider = "local"` — online when the model is installed, else the
    /// in-chat line names the fetch command (never a panic).
    #[cfg(feature = "local-embed")]
    #[test]
    fn embed_slot_local() {
        let slot = EmbedSlot::from_lookup(
            |k| (k == "embed.provider").then(|| PrefValue::Str("local".into())),
            none,
        );
        let installed = gneiss_pal::api::local::model_dir("all-MiniLM-L6-v2").join("model.onnx").is_file();
        if installed {
            assert_eq!(slot.status_line(), ":: BRAIN :: EMBED local/all-MiniLM-L6-v2 dims=384\n\n");
            let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
            let (e, why) = rt.block_on(slot.embed_one("semantic recall with no network"));
            assert_eq!((e.vector.len(), e.tag.as_str(), why), (384, "local/all-MiniLM-L6-v2", None));
        } else {
            assert!(slot.status_line().contains("run `tools/una-models fetch all-MiniLM-L6-v2`"), "{}", slot.status_line());
        }
    }
}
