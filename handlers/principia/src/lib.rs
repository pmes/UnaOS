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

//! `principia` — the Architect: the policy engine, and the OS settings surface.
//!
//! Principia owns **preferences**: the namespaced, typed, persistent settings
//! every other part of userspace reads. It is headless like every handler — it
//! answers on the [`bandy`] Synapse and never draws anything.
//!
//! Two capabilities live here today:
//!
//! - **The preference store** ([`prefs::PrefStore`]) — `~/settings/<domain>`, one file per
//!   domain (`prefs_core::files`, R98), the SAME folder the kernel reads and writes
//!   (PRINCIPIAFILES, rmbp-ledger B445): addressed by namespace + dotted key, four
//!   scalar types, atomically written. The pre-R98 `~/.config/unaos/preferences.toml`
//!   is migrated into it once and deleted.
//!   Defaults belong to the consumer; a `get` on an unset key answers `None`.
//! - **The system root** — the original capability, unchanged:
//!   `~/.config/unaos/principia.toml` holds the workspace path the rest of the
//!   system operates against.
//!
//! # Bus verbs
//!
//! All under [`SMessage::Principia`]:
//! `PrefGet` → `PrefValueIs`, `PrefList` → `PrefListIs`, `PrefSet` →
//! `PrefChanged` (broadcast, the live-update signal) or `PrefError`, and the
//! pre-existing `SetSystemRoot` → `SystemRootChanged`.
//!
//! # Not implemented (deliberately, this round)
//!
//! - **Reload on external change.** Principia is the writer of record; an edit
//!   made to the file underneath a running Principia is not noticed until the
//!   next load. A watcher is queued.
//! - **Policy levels.** The charter's law layer (the safety levels helm
//!   enforces) rides this same store when the first drivable domain lands; the
//!   store is the mechanism, the levels are not yet defined.

pub mod prefs;
pub mod wire;

use std::fs;
use std::path::{Path, PathBuf};

use bandy::{PrefValue, PrincipiaCommand, SMessage, Synapse};

pub use prefs::{PrefStore, SetOutcome};

/// The Architect's live state: the preference store plus the system root.
pub struct Principia {
    current_root: Option<PathBuf>,
    config_path: PathBuf,
    prefs: PrefStore,
}

impl Principia {
    /// Open Principia against the standard config lobe
    /// (`~/.config/unaos/`, via `dirs::config_dir()`) for the system root and
    /// the home's settings folder ([`default_prefs_path`]), creating the lobe if needed.
    pub fn new() -> Self {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("unaos");
        fs::create_dir_all(&config_dir).expect("Failed to create Principia config lobe");

        Self::with_dirs(&config_dir, &default_prefs_path())
    }

    /// Open Principia against an explicit HOME (`<home>/.config/unaos` for the
    /// system root, `<home>/settings` for the preferences) — the seam the tests
    /// (and any future multi-profile boot) use.
    pub fn with_home(home: &Path) -> Self {
        Self::with_dirs(&home.join(".config").join("unaos"), &home.join(prefs_core::files::DIR))
    }

    /// Open Principia with the system-root lobe and the settings folder named apart.
    /// Principia is the host's writer of record: the load is SETTLED here (the
    /// pre-R98 single file migrated and deleted, a load-time clamp re-saved).
    pub fn with_dirs(config_dir: &Path, settings_dir: &Path) -> Self {
        let config_path = config_dir.join("principia.toml");
        let current_root = fs::read_to_string(&config_path)
            .ok()
            .map(|s| PathBuf::from(s.trim()));

        let prefs = match PrefStore::load(settings_dir) {
            Ok(mut p) => {
                if let Err(e) = p.settle() {
                    log::error!("[PRINCIPIA] :: settling {} failed ({e:#}); the old file stays", settings_dir.display());
                }
                p
            }
            Err(e) => {
                // An unreadable folder must not take the whole handler down, and
                // must not be silently overwritten either: serve an empty store
                // bound to a quarantine folder, and say so loudly.
                log::error!(
                    "[PRINCIPIA] :: {} is unreadable ({e:#}); serving an empty store and \
                     writing to {}.new — the original is untouched.",
                    settings_dir.display(),
                    settings_dir.display()
                );
                let mut q = settings_dir.as_os_str().to_owned();
                q.push(".new");
                PrefStore::empty(PathBuf::from(q))
            }
        };

        Self {
            current_root,
            config_path,
            prefs,
        }
    }

    /// The workspace root currently in force, if one has been set.
    pub fn current_root(&self) -> Option<&Path> {
        self.current_root.as_deref()
    }

    /// Read-only access to the preference store (for an in-process consumer
    /// that holds the handler directly rather than speaking on the bus).
    pub fn prefs(&self) -> &PrefStore {
        &self.prefs
    }

    /// The value in force for `ns`/`key` under the process environment:
    /// stored, else the schema default, else its derived [`prefs_core::rules::Rule`]
    /// (R81's embedder: gemini when the Gemini key variable is set, else local
    /// when the local model is installed, else off). The bus `PrefGet` keeps
    /// answering the raw store (`None` = unset), byte for byte the kernel's
    /// PREF_GET.
    pub fn effective(&self, ns: &str, key: &str) -> Option<(PrefValue, prefs_core::schema::Source)> {
        self.prefs.effective(ns, key, |k| std::env::var(k).ok())
    }

    /// The Synaptic Receiver: one inbound message in, at most one outbound
    /// message out. The caller publishes what comes back.
    pub fn process_impulse(&mut self, msg: &SMessage) -> Option<SMessage> {
        let SMessage::Principia(cmd) = msg else {
            return None;
        };

        match cmd {
            PrincipiaCommand::SetSystemRoot(path) => {
                if self.validate_root(path) {
                    self.current_root = Some(path.clone());
                    if let Some(lobe) = self.config_path.parent() {
                        let _ = fs::create_dir_all(lobe);
                    }
                    let _ = fs::write(&self.config_path, path.to_string_lossy().as_ref());

                    // Fire the echo back across the bus
                    return Some(SMessage::Principia(PrincipiaCommand::SystemRootChanged(
                        path.clone(),
                    )));
                }
                None
            }

            PrincipiaCommand::PrefGet { ns, key } => {
                Some(SMessage::Principia(PrincipiaCommand::PrefValueIs {
                    ns: ns.clone(),
                    key: key.clone(),
                    value: self.prefs.get(ns, key),
                }))
            }

            PrincipiaCommand::PrefList { ns } => {
                Some(SMessage::Principia(PrincipiaCommand::PrefListIs {
                    ns: ns.clone(),
                    entries: self.prefs.list(ns),
                }))
            }

            PrincipiaCommand::PrefSet { ns, key, value } => Some(self.set_pref(ns, key, value)),

            // Replies and broadcasts are not commands — Principia hears its own
            // output on the broadcast Synapse and must not act on it.
            PrincipiaCommand::SystemRootChanged(_)
            | PrincipiaCommand::PrefValueIs { .. }
            | PrincipiaCommand::PrefListIs { .. }
            | PrincipiaCommand::PrefChanged { .. }
            | PrincipiaCommand::PrefError { .. } => None,
        }
    }

    /// Apply one set: persist, then answer with the broadcast `PrefChanged`
    /// (the acknowledgement *and* the live-update signal) or with `PrefError`.
    fn set_pref(&mut self, ns: &str, key: &str, value: &PrefValue) -> SMessage {
        match self.prefs.set(ns, key, value.clone()) {
            Ok(out) => {
                log::info!(
                    "[PRINCIPIA] :: {ns}.{key} = {} ({}){}",
                    prefs::to_core(&out.value),
                    out.value.type_name(),
                    if out.clamped { " clamped=true" } else { "" }
                );
                SMessage::Principia(PrincipiaCommand::PrefChanged {
                    ns: ns.to_string(),
                    key: key.to_string(),
                    value: out.value,
                    clamped: out.clamped,
                })
            }
            Err(e) => {
                log::warn!("[PRINCIPIA] :: rejected {ns}.{key}: {e:#}");
                SMessage::Principia(PrincipiaCommand::PrefError {
                    ns: ns.to_string(),
                    key: key.to_string(),
                    message: format!("{e:#}"),
                })
            }
        }
    }

    #[inline]
    fn validate_root(&self, path: &Path) -> bool {
        // A valid UnaOS root must have a crates or libs directory
        path.exists()
            && path.is_dir()
            && (path.join("crates").exists() || path.join("libs").exists())
    }
}

impl Default for Principia {
    fn default() -> Self {
        Self::new()
    }
}

/// Ignite Principia on the Synapse: subscribe, serve every inbound command,
/// publish every answer. Runs until the Synapse's last sender is dropped.
///
/// **Must be called from within a Tokio runtime** (the usual handler shape:
/// `tokio::spawn(principia::ignite(synapse.clone()))`).
pub async fn ignite(synapse: Synapse) {
    ignite_with(synapse, Principia::new()).await
}

/// [`ignite`] against an already-constructed handler — the seam for a vessel
/// that wants a non-default config lobe.
///
/// Subscription happens when the returned future is first polled, so a caller
/// that fires a command immediately after `tokio::spawn` can outrun it. Use
/// [`serve`] with a receiver taken before the spawn when the ordering matters.
pub async fn ignite_with(synapse: Synapse, principia: Principia) {
    let rx = synapse.subscribe();
    serve(synapse, rx, principia).await
}

/// The serving loop against a receiver the caller already holds — subscribe
/// first, spawn second, and no command fired in between can be missed.
pub async fn serve(
    synapse: Synapse,
    mut rx: tokio::sync::broadcast::Receiver<SMessage>,
    mut principia: Principia,
) {
    log::info!(
        "[PRINCIPIA] :: Policy engine live; preferences at {}",
        principia.prefs.path().display()
    );

    loop {
        match rx.recv().await {
            Ok(msg) => {
                if let Some(reply) = principia.process_impulse(&msg) {
                    synapse.fire(reply);
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                // A dropped command is a lost setting change; say so rather
                // than let it vanish.
                log::warn!("[PRINCIPIA] :: lagged {n} messages behind the Synapse");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                log::info!("[PRINCIPIA] :: Synapse closed; policy engine terminating.");
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TESTS — the handler face and the bus round trip (no GUI, no host config lobe)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn handler(dir: &tempfile::TempDir) -> Principia {
        Principia::with_home(dir.path())
    }

    fn pref_reply(msg: Option<SMessage>) -> PrincipiaCommand {
        match msg {
            Some(SMessage::Principia(cmd)) => cmd,
            other => panic!("expected a Principia reply, got {other:?}"),
        }
    }

    #[test]
    fn get_of_an_unset_key_answers_none_not_a_default() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = handler(&dir);
        let reply = pref_reply(p.process_impulse(&SMessage::Principia(
            PrincipiaCommand::PrefGet {
                ns: "aether".into(),
                key: "homepage".into(),
            },
        )));
        match reply {
            PrincipiaCommand::PrefValueIs { ns, key, value } => {
                assert_eq!(ns, "aether");
                assert_eq!(key, "homepage");
                assert_eq!(value, None);
            }
            other => panic!("expected PrefValueIs, got {other:?}"),
        }
    }

    #[test]
    fn set_then_get_round_trips_through_the_handler() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = handler(&dir);

        let changed = pref_reply(p.process_impulse(&SMessage::Principia(
            PrincipiaCommand::PrefSet {
                ns: "aether".into(),
                key: "window.width".into(),
                value: PrefValue::Int(1280),
            },
        )));
        match changed {
            PrincipiaCommand::PrefChanged { ns, key, value, .. } => {
                assert_eq!((ns.as_str(), key.as_str()), ("aether", "window.width"));
                assert_eq!(value, PrefValue::Int(1280));
            }
            other => panic!("a successful set must answer PrefChanged, got {other:?}"),
        }

        let got = pref_reply(p.process_impulse(&SMessage::Principia(
            PrincipiaCommand::PrefGet {
                ns: "aether".into(),
                key: "window.width".into(),
            },
        )));
        match got {
            PrincipiaCommand::PrefValueIs { value, .. } => {
                assert_eq!(value, Some(PrefValue::Int(1280)))
            }
            other => panic!("expected PrefValueIs, got {other:?}"),
        }

        // ...and it survives a restart of the handler against the same lobe.
        let mut p2 = handler(&dir);
        let got = pref_reply(p2.process_impulse(&SMessage::Principia(
            PrincipiaCommand::PrefGet {
                ns: "aether".into(),
                key: "window.width".into(),
            },
        )));
        match got {
            PrincipiaCommand::PrefValueIs { value, .. } => {
                assert_eq!(value, Some(PrefValue::Int(1280)), "preferences must persist")
            }
            other => panic!("expected PrefValueIs, got {other:?}"),
        }
    }

    #[test]
    fn list_answers_one_namespace_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = handler(&dir);
        for (ns, key, value) in [
            ("aether", "homepage", PrefValue::Str("https://una.os/".into())),
            ("aether", "window.width", PrefValue::Int(1280)),
            ("stria", "muted", PrefValue::Bool(true)),
        ] {
            p.process_impulse(&SMessage::Principia(PrincipiaCommand::PrefSet {
                ns: ns.into(),
                key: key.into(),
                value,
            }));
        }

        let listed = pref_reply(p.process_impulse(&SMessage::Principia(
            PrincipiaCommand::PrefList { ns: "aether".into() },
        )));
        match listed {
            PrincipiaCommand::PrefListIs { ns, entries } => {
                assert_eq!(ns, "aether");
                assert_eq!(
                    entries,
                    vec![
                        ("homepage".to_string(), PrefValue::Str("https://una.os/".into())),
                        ("window.width".to_string(), PrefValue::Int(1280)),
                    ]
                );
            }
            other => panic!("expected PrefListIs, got {other:?}"),
        }
    }

    #[test]
    fn a_rejected_set_answers_pref_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = handler(&dir);
        let reply = pref_reply(p.process_impulse(&SMessage::Principia(
            PrincipiaCommand::PrefSet {
                ns: "aether".into(),
                key: "window..width".into(),
                value: PrefValue::Int(1),
            },
        )));
        match reply {
            PrincipiaCommand::PrefError { ns, key, message } => {
                assert_eq!((ns.as_str(), key.as_str()), ("aether", "window..width"));
                assert!(!message.is_empty(), "an error must say what was wrong");
            }
            other => panic!("expected PrefError, got {other:?}"),
        }
    }

    /// PRINCIPIA2 (SR32, B300 owed): the host `PrefSet` clamps a declared key
    /// through the SAME prefs_core function the kernel's Settings path uses,
    /// stores the clamp, and answers it with `clamped = true`.
    #[test]
    fn an_out_of_range_set_clamps_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = handler(&dir);
        let set = |p: &mut Principia, key: &str, value: PrefValue| {
            pref_reply(p.process_impulse(&SMessage::Principia(PrincipiaCommand::PrefSet {
                ns: "system".into(),
                key: key.into(),
                value,
            })))
        };
        match set(&mut p, "display.brightness", PrefValue::Int(0)) {
            PrincipiaCommand::PrefChanged { value, clamped, .. } => {
                assert_eq!(value, PrefValue::Int(prefs_core::display::clamp_brightness(0)));
                assert!(clamped, "a dark brightness is clamped and said");
            }
            other => panic!("expected PrefChanged, got {other:?}"),
        }
        match set(&mut p, "audio.volume", PrefValue::Int(99)) {
            PrincipiaCommand::PrefChanged { value, clamped, .. } => {
                assert_eq!((value, clamped), (PrefValue::Int(16), true));
            }
            other => panic!("expected PrefChanged, got {other:?}"),
        }
        match set(&mut p, "audio.volume", PrefValue::Int(9)) {
            PrincipiaCommand::PrefChanged { value, clamped, .. } => {
                assert_eq!((value, clamped), (PrefValue::Int(9), false));
            }
            other => panic!("expected PrefChanged, got {other:?}"),
        }
        // The clamp is what persisted.
        let back = PrefStore::load(dir.path().join(prefs_core::files::DIR)).unwrap();
        assert_eq!(back.get("system", "display.brightness"), Some(PrefValue::Int(1)));
        // A wrong type is refused, and the store is untouched.
        match set(&mut p, "audio.volume", PrefValue::Str("loud".into())) {
            PrincipiaCommand::PrefError { message, .. } => assert!(message.contains("expected int"), "{message}"),
            other => panic!("expected PrefError, got {other:?}"),
        }
        assert_eq!(p.prefs().get("system", "audio.volume"), Some(PrefValue::Int(9)));
    }

    /// Principia hears its own broadcasts on the Synapse; a reply must never
    /// provoke another reply (an echo storm on a broadcast bus).
    #[test]
    fn replies_and_broadcasts_are_inert() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = handler(&dir);
        for cmd in [
            PrincipiaCommand::PrefChanged {
                ns: "aether".into(),
                key: "homepage".into(),
                value: PrefValue::Str("x".into()),
                clamped: false,
            },
            PrincipiaCommand::PrefValueIs {
                ns: "aether".into(),
                key: "homepage".into(),
                value: None,
            },
            PrincipiaCommand::PrefListIs {
                ns: "aether".into(),
                entries: vec![],
            },
            PrincipiaCommand::PrefError {
                ns: "aether".into(),
                key: "k".into(),
                message: "m".into(),
            },
            PrincipiaCommand::SystemRootChanged(PathBuf::from("/una")),
        ] {
            assert!(
                p.process_impulse(&SMessage::Principia(cmd)).is_none(),
                "a reply/broadcast must not produce another message"
            );
        }
        // Foreign traffic is ignored just as completely.
        assert!(p.process_impulse(&SMessage::Ping).is_none());
    }

    /// The original capability still works, and still persists.
    #[test]
    fn set_system_root_still_validates_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(root.join("libs")).unwrap();
        let mut p = handler(&dir);

        assert!(
            p.process_impulse(&SMessage::Principia(PrincipiaCommand::SetSystemRoot(
                dir.path().join("not-a-tree")
            )))
            .is_none(),
            "a path that is not a UnaOS tree must be refused"
        );

        let reply = pref_reply(p.process_impulse(&SMessage::Principia(
            PrincipiaCommand::SetSystemRoot(root.clone()),
        )));
        match reply {
            PrincipiaCommand::SystemRootChanged(p) => assert_eq!(p, root),
            other => panic!("expected SystemRootChanged, got {other:?}"),
        }
        assert_eq!(handler(&dir).current_root(), Some(root.as_path()));
    }

    /// The bus-level proof: a live Synapse, the real ignite loop, a set that
    /// comes back as a broadcast PrefChanged, and a get that answers from the
    /// store — exactly the traffic a consumer will speak.
    #[tokio::test]
    async fn bus_round_trip_over_a_live_synapse() {
        let dir = tempfile::tempdir().unwrap();
        let synapse = Synapse::new();
        // Subscribe BEFORE ignition — both the observer and the handler — so
        // nothing fired below can be missed.
        let mut rx = synapse.subscribe();
        let handler_rx = synapse.subscribe();
        let handler = Principia::with_home(dir.path());
        tokio::spawn(serve(synapse.clone(), handler_rx, handler));

        synapse.fire(SMessage::Principia(PrincipiaCommand::PrefSet {
            ns: "aether".into(),
            key: "homepage".into(),
            value: PrefValue::Str("https://una.os/".into()),
        }));

        let changed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let SMessage::Principia(PrincipiaCommand::PrefChanged { ns, key, value, .. }) =
                    rx.recv().await.unwrap()
                {
                    return (ns, key, value);
                }
            }
        })
        .await
        .expect("a set must broadcast PrefChanged");
        assert_eq!(changed.0, "aether");
        assert_eq!(changed.1, "homepage");
        assert_eq!(changed.2, PrefValue::Str("https://una.os/".into()));

        synapse.fire(SMessage::Principia(PrincipiaCommand::PrefGet {
            ns: "aether".into(),
            key: "homepage".into(),
        }));

        let value = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let SMessage::Principia(PrincipiaCommand::PrefValueIs { value, .. }) =
                    rx.recv().await.unwrap()
                {
                    return value;
                }
            }
        })
        .await
        .expect("a get must be answered");
        assert_eq!(value, Some(PrefValue::Str("https://una.os/".into())));

        // The set really landed on disk, not just in the cache.
        assert!(dir.path().join("settings").join("aether").exists());
    }
}

/// The standard preference store, the folder `~/settings` (via
/// `dirs::home_dir()`; `prefs_core::files::DIR`, the kernel's `<home>/settings`):
/// the folder [`Principia::new`] serves. A reader that is not the store's
/// writer (Vein reading its `vein` namespace, VEINPROV B303) opens it
/// read-only through [`prefs::PrefStore::load`].
pub fn default_prefs_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(prefs_core::files::DIR)
}

