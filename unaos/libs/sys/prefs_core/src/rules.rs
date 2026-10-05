// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! DEFAULT RULES — derived defaults (PRINCIPIA2, SR32). A [`Rule`] computes a key's default from other
//! preferences and from the machine (an environment variable is set; a local model is installed). The
//! core cannot see either, so the caller supplies them through [`RuleEnv`]: Principia on the host
//! (`std::env`, the model cache), the kernel when it has a consumer for one.

use alloc::string::String;

use crate::schema::{self, CLAUDECODE_DEFAULT_MODEL, CLAUDE_DEFAULT_MODEL, GEMINI_DEFAULT_KEY_ENV, GEMINI_DEFAULT_MODEL, LOCAL_EMBED_MODEL};
use crate::PrefValue;

/// What a rule may look at.
pub trait RuleEnv {
    /// The STORED value of `ns`/`key` (never a default).
    fn pref(&self, ns: &str, key: &str) -> Option<PrefValue>;
    /// The environment variable `var` is set to a non-blank value.
    fn env_set(&self, var: &str) -> bool;
    /// The local model `name` is installed (every file of the `tools/una-models` manifest present).
    fn local_model_installed(&self, name: &str) -> bool;
}

/// A derived default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// R81: `vein.embed.provider` = `gemini` when `vein.gemini.api_key_env` (default `GEMINI_API_KEY`)
    /// names a set variable, else `local` when the local model is installed, else `off`.
    Embedder,
    /// R81: `vein.model` = `claude-opus-5-5` for the claude provider, `gemini-3.1-pro-preview` for
    /// gemini; no model for the metal's echo / relay.
    ChatModel,
}

/// A non-blank stored string, trimmed — the consumers' `pref_str`.
fn stored_str(env: &dyn RuleEnv, ns: &str, key: &str) -> Option<String> {
    match env.pref(ns, key) {
        Some(PrefValue::Str(s)) if !s.trim().is_empty() => Some(String::from(s.trim())),
        _ => None,
    }
}

impl Rule {
    pub const ALL: [Rule; 2] = [Rule::Embedder, Rule::ChatModel];

    pub const fn name(self) -> &'static str {
        match self {
            Rule::Embedder => "embedder",
            Rule::ChatModel => "chat-model",
        }
    }

    pub const fn describe(self) -> &'static str {
        match self {
            Rule::Embedder => "`vein.embed.provider`: `gemini` when `vein.gemini.api_key_env` (default `GEMINI_API_KEY`) names a set, non-blank environment variable; else `local` when the local model `all-MiniLM-L6-v2` is installed (every file of the `tools/una-models` manifest in `${XDG_CACHE_HOME:-$HOME/.cache}/unaos/models/all-MiniLM-L6-v2/`); else `off` (R81).",
            Rule::ChatModel => "`vein.model`: `claude-opus-5-5` when `vein.provider` is `claude` (its default), `gemini-3.1-pro-preview` when `gemini`; `default` (the CLI's own model) when `claudecode`; none for `echo` / `relay` (R81).",
        }
    }

    /// Evaluate under `env`. `None` = the rule yields no value.
    pub fn eval(self, env: &dyn RuleEnv) -> Option<PrefValue> {
        match self {
            Rule::Embedder => {
                let var = stored_str(env, "vein", "gemini.api_key_env").unwrap_or_else(|| String::from(GEMINI_DEFAULT_KEY_ENV));
                let pick = if env.env_set(&var) {
                    "gemini"
                } else if env.local_model_installed(LOCAL_EMBED_MODEL) {
                    "local"
                } else {
                    "off"
                };
                Some(PrefValue::Str(String::from(pick)))
            }
            Rule::ChatModel => {
                let provider = match schema::effective("vein", "provider", env) {
                    Some((PrefValue::Str(s), _)) => s,
                    _ => return None,
                };
                match provider.trim() {
                    "claude" => Some(PrefValue::Str(String::from(CLAUDE_DEFAULT_MODEL))),
                    "gemini" => Some(PrefValue::Str(String::from(GEMINI_DEFAULT_MODEL))),
                    "claudecode" => Some(PrefValue::Str(String::from(CLAUDECODE_DEFAULT_MODEL))),
                    _ => None,
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::schema::{default_of, effective, Source};
    use alloc::collections::BTreeMap;
    use alloc::vec::Vec;

    /// A scripted environment.
    #[derive(Default)]
    pub struct Env {
        pub prefs: BTreeMap<(String, String), PrefValue>,
        pub vars: Vec<String>,
        pub models: Vec<String>,
    }
    impl Env {
        pub fn with(mut self, ns: &str, key: &str, v: PrefValue) -> Self {
            self.prefs.insert((ns.into(), key.into()), v);
            self
        }
        pub fn var(mut self, v: &str) -> Self {
            self.vars.push(v.into());
            self
        }
        pub fn model(mut self, m: &str) -> Self {
            self.models.push(m.into());
            self
        }
    }
    impl RuleEnv for Env {
        fn pref(&self, ns: &str, key: &str) -> Option<PrefValue> {
            self.prefs.get(&(ns.into(), key.into())).cloned()
        }
        fn env_set(&self, var: &str) -> bool {
            self.vars.iter().any(|v| v == var)
        }
        fn local_model_installed(&self, name: &str) -> bool {
            self.models.iter().any(|m| m == name)
        }
    }

    fn embedder(env: &Env) -> Option<(PrefValue, Source)> {
        effective("vein", "embed.provider", env)
    }
    fn s(x: &str) -> PrefValue {
        PrefValue::Str(x.into())
    }

    #[test]
    fn embedder_gemini_when_the_default_key_variable_is_set() {
        let env = Env::default().var("GEMINI_API_KEY");
        assert_eq!(embedder(&env), Some((s("gemini"), Source::Rule(Rule::Embedder))));
        // ...even with a local model installed: a configured key wins.
        assert_eq!(embedder(&env.model(LOCAL_EMBED_MODEL)).unwrap().0, s("gemini"));
    }

    #[test]
    fn embedder_follows_a_renamed_key_variable() {
        let env = Env::default().with("vein", "gemini.api_key_env", s("MY_GEM")).var("MY_GEM");
        assert_eq!(embedder(&env).unwrap().0, s("gemini"));
        // The default name no longer counts once the preference names another variable.
        let env = Env::default().with("vein", "gemini.api_key_env", s("MY_GEM")).var("GEMINI_API_KEY");
        assert_eq!(embedder(&env).unwrap().0, s("off"));
        // A blank preference falls back to the default name.
        let env = Env::default().with("vein", "gemini.api_key_env", s("  ")).var("GEMINI_API_KEY");
        assert_eq!(embedder(&env).unwrap().0, s("gemini"));
    }

    #[test]
    fn embedder_local_when_no_key_and_the_model_is_installed() {
        let env = Env::default().model(LOCAL_EMBED_MODEL);
        assert_eq!(embedder(&env), Some((s("local"), Source::Rule(Rule::Embedder))));
        // Another model in the cache is not THE local embedder.
        assert_eq!(embedder(&Env::default().model("bge-small")).unwrap().0, s("off"));
    }

    #[test]
    fn embedder_off_when_neither() {
        assert_eq!(embedder(&Env::default()), Some((s("off"), Source::Rule(Rule::Embedder))));
    }

    #[test]
    fn a_stored_embedder_wins_over_the_rule() {
        let env = Env::default().var("GEMINI_API_KEY").with("vein", "embed.provider", s("off"));
        assert_eq!(embedder(&env), Some((s("off"), Source::Stored)));
    }

    #[test]
    fn chat_provider_defaults_to_claude_and_the_model_follows() {
        let env = Env::default();
        assert_eq!(effective("vein", "provider", &env), Some((s("claude"), Source::Default)));
        assert_eq!(default_of("vein", "model", &env), Some((s(CLAUDE_DEFAULT_MODEL), Source::Rule(Rule::ChatModel))));
        let g = Env::default().with("vein", "provider", s("gemini"));
        assert_eq!(default_of("vein", "model", &g).unwrap().0, s(GEMINI_DEFAULT_MODEL));
        let c = Env::default().with("vein", "provider", s("claudecode"));
        assert_eq!(default_of("vein", "model", &c).unwrap().0, s(CLAUDECODE_DEFAULT_MODEL));
        let e = Env::default().with("vein", "provider", s("echo"));
        assert_eq!(default_of("vein", "model", &e), None);
        // A Gemini key alone does not move the CHAT provider (R81: never a hardwired Gemini default).
        assert_eq!(effective("vein", "provider", &Env::default().var("GEMINI_API_KEY")).unwrap().0, s("claude"));
    }
}
