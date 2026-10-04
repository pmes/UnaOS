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

//! The preference store: namespaced, typed, TOML-backed, atomically written.
//!
//! # Shape
//!
//! A preference is addressed by a **namespace** (a per-app/domain string —
//! `"aether"`, `"stria"`, `"system"`) and a **dotted key** within it
//! (`"homepage"`, `"window.width"`). The value is one of four scalar types
//! ([`PrefValue`]) — exactly what a TOML scalar carries losslessly, so nothing
//! is retyped by a save/load cycle.
//!
//! On disk that maps to the obvious hand-editable TOML: one table per
//! namespace, dotted keys expanded into sub-tables.
//!
//! ```toml
//! [aether]
//! homepage = "https://una.os/"
//!
//! [aether.window]
//! width = 1280
//! height = 800
//! ```
//!
//! # Rules
//!
//! - **Defaults live with the consumer.** The store never invents a value;
//!   [`PrefStore::get`] answers `Option`, and an unset key is simply unset.
//! - **Every write is atomic.** A set serializes the whole document into a
//!   sibling temp file, fsyncs it, and `rename`s it over the real one — a
//!   reader (or a crash) sees the old file or the new one, never a partial.
//! - **A key is a leaf.** `window` and `window.width` cannot both hold values,
//!   because TOML cannot express it; the collision is rejected at set time
//!   rather than at save time, so the in-memory state never diverges from what
//!   is persistable.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use bandy::PrefValue;

/// One namespace's flat key → value map (sorted: a stable file diff).
type Namespace = BTreeMap<String, PrefValue>;

/// What a [`PrefStore::set`] stored.
#[derive(Debug, Clone, PartialEq)]
pub struct SetOutcome {
    /// The value stored: the written one, or its clamp.
    pub value: PrefValue,
    /// `true` when the written value was outside the key's declared range and
    /// [`SetOutcome::value`] is the clamp.
    pub clamped: bool,
}

/// The namespaced preference store, cached in memory and backed by a TOML file.
///
/// Reload-on-external-change is **not** implemented: the store is the writer of
/// record, and an edit made to the file underneath a running Principia is not
/// noticed until the next load. (Queued — see the README.)
pub struct PrefStore {
    path: PathBuf,
    namespaces: BTreeMap<String, Namespace>,
}

impl PrefStore {
    /// Load the store from `path`. A missing file is an empty store, not an
    /// error — first boot has no preferences. A malformed file IS an error:
    /// silently starting empty would let a save overwrite a user's settings
    /// with nothing.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let namespaces = match fs::read_to_string(&path) {
            Ok(text) => parse_document(&text)
                .with_context(|| format!("parsing preferences at {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => {
                return Err(e).with_context(|| format!("reading {}", path.display()));
            }
        };
        Ok(Self { path, namespaces })
    }

    /// An empty store bound to `path`, without reading anything — the honest
    /// fallback when the real file cannot be trusted (see
    /// `Principia::with_config_dir`'s quarantine path).
    pub fn empty(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            namespaces: BTreeMap::new(),
        }
    }

    /// The file this store persists to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The value of `ns`/`key`, or `None` if unset. The caller owns the
    /// default.
    pub fn get(&self, ns: &str, key: &str) -> Option<PrefValue> {
        self.namespaces.get(ns)?.get(key).cloned()
    }

    /// Every `(key, value)` set in `ns`, sorted by key. An unknown namespace
    /// lists empty — the same answer as a namespace with nothing set.
    pub fn list(&self, ns: &str) -> Vec<(String, PrefValue)> {
        self.namespaces
            .get(ns)
            .map(|n| n.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    /// Every namespace that currently holds at least one preference.
    pub fn namespaces(&self) -> Vec<String> {
        self.namespaces.keys().cloned().collect()
    }

    /// The value IN FORCE for `ns`/`key`: the stored one, else the schema's
    /// default, else the schema's derived default ([`prefs_core::rules::Rule`],
    /// e.g. R81's embedder). `env` answers environment variables (the process
    /// environment in [`crate::Principia::effective`]; a script in tests); the
    /// local-model cache is found from `XDG_CACHE_HOME` / `HOME` through it.
    /// [`PrefStore::get`] stays the raw store: an unset key there is `None`.
    pub fn effective(
        &self,
        ns: &str,
        key: &str,
        env: impl Fn(&str) -> Option<String>,
    ) -> Option<(PrefValue, prefs_core::schema::Source)> {
        let host = HostEnv { store: self, env };
        prefs_core::schema::effective(ns, key, &host).map(|(v, src)| (from_core(&v), src))
    }

    /// Set `ns`/`key` and persist the whole document atomically.
    ///
    /// The write first goes through the ONE schema validator both rings run,
    /// `prefs_core::schema::check` (PRINCIPIA2, SR32): a declared key written
    /// out of range is CLAMPED (the answer says so: [`SetOutcome::clamped`]);
    /// a wrong type, a value outside the key's enum, an over-long or
    /// unprintable string is refused. An undeclared key is stored as given.
    ///
    /// Errors on a malformed namespace/key, a schema refusal, a key that
    /// collides with an existing dotted path, or a failed write — and on error
    /// the in-memory cache is left exactly as it was, so cache and file never
    /// disagree.
    pub fn set(&mut self, ns: &str, key: &str, value: PrefValue) -> Result<SetOutcome> {
        validate_ns(ns)?;
        validate_key(key)?;
        let applied = prefs_core::schema::check(ns, key, to_core(&value))
            .map_err(|r| anyhow::anyhow!("refused `{ns}`.`{key}` = {}: {r}", to_core(&value)))?;
        let outcome = SetOutcome { value: from_core(&applied.value), clamped: applied.clamped };
        let value = outcome.value.clone();

        let entry = self.namespaces.entry(ns.to_string()).or_default();
        if let Some(other) = colliding_key(entry, key) {
            bail!(
                "key `{key}` collides with `{other}` in namespace `{ns}`: \
                 one cannot be both a value and a table"
            );
        }

        let previous = entry.insert(key.to_string(), value);
        if let Err(e) = self.persist() {
            // Roll the cache back to the persisted truth.
            let entry = self.namespaces.entry(ns.to_string()).or_default();
            match previous {
                Some(old) => {
                    entry.insert(key.to_string(), old);
                }
                None => {
                    entry.remove(key);
                    if entry.is_empty() {
                        self.namespaces.remove(ns);
                    }
                }
            }
            return Err(e);
        }
        Ok(outcome)
    }

    /// Serialize the store and replace the file atomically: write a sibling
    /// temp file, flush + fsync it, then `rename` it into place.
    fn persist(&self) -> Result<()> {
        let text = self.to_toml()?;

        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }

        // Same directory as the target, so the rename is within one filesystem
        // (a cross-device rename is not atomic and would fail outright).
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(format!(".tmp.{}", std::process::id()));
        let tmp = PathBuf::from(tmp);

        {
            let mut f = fs::File::create(&tmp)
                .with_context(|| format!("creating {}", tmp.display()))?;
            f.write_all(text.as_bytes())
                .with_context(|| format!("writing {}", tmp.display()))?;
            f.sync_all()
                .with_context(|| format!("syncing {}", tmp.display()))?;
        }

        fs::rename(&tmp, &self.path).with_context(|| {
            format!("renaming {} onto {}", tmp.display(), self.path.display())
        })?;
        Ok(())
    }

    /// The whole store as TOML text (dotted keys expanded into sub-tables).
    pub fn to_toml(&self) -> Result<String> {
        let mut doc = toml::Table::new();
        for (ns, entries) in &self.namespaces {
            let mut table = toml::Table::new();
            for (key, value) in entries {
                insert_dotted(&mut table, key, to_toml_value(value))
                    .with_context(|| format!("serializing `{ns}`.`{key}`"))?;
            }
            doc.insert(ns.clone(), toml::Value::Table(table));
        }
        let body = toml::to_string_pretty(&doc).context("serializing preferences")?;
        Ok(format!("{HEADER}{body}"))
    }
}

const HEADER: &str = "\
# UnaOS preferences — written by the principia handler.
# One table per namespace; dotted keys are expanded into sub-tables.
# Hand edits are read on next load (live reload is not implemented yet).

";

// ---------------------------------------------------------------------------
// DEFAULT RULES — what a prefs_core rule may see on the host
// ---------------------------------------------------------------------------

/// `${XDG_CACHE_HOME:-$HOME/.cache}/unaos/models` — where `tools/una-models`
/// installs (the same path `gneiss_pal::api::local::model_dir` reads).
pub fn models_dir(env: &impl Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"));
    base.join("unaos").join("models")
}

/// The store plus an environment, as a [`prefs_core::rules::RuleEnv`].
struct HostEnv<'a, E: Fn(&str) -> Option<String>> {
    store: &'a PrefStore,
    env: E,
}

impl<E: Fn(&str) -> Option<String>> prefs_core::rules::RuleEnv for HostEnv<'_, E> {
    fn pref(&self, ns: &str, key: &str) -> Option<prefs_core::PrefValue> {
        self.store.get(ns, key).map(|v| to_core(&v))
    }
    fn env_set(&self, var: &str) -> bool {
        (self.env)(var).is_some_and(|v| !v.trim().is_empty())
    }
    /// Installed = every file of the `tools/una-models` manifest present.
    fn local_model_installed(&self, name: &str) -> bool {
        let dir = models_dir(&self.env).join(name);
        prefs_core::schema::LOCAL_MODEL_FILES.iter().all(|f| dir.join(f).is_file())
    }
}

// ---------------------------------------------------------------------------
// VALIDATION
// ---------------------------------------------------------------------------

// The rule itself lives in `prefs_core` (PREFS, rmbp-ledger B300) so the kernel, which reads and
// writes this same file, cannot drift from it; these keep Principia's `anyhow` messages.

/// A namespace is a single bare-key segment: `aether`, `stria`, `system`.
pub fn validate_ns(ns: &str) -> Result<()> {
    if prefs_core::validate_ns(ns).is_err() {
        bail!(
            "invalid namespace `{ns}`: expected a non-empty \
             [A-Za-z0-9_-] identifier"
        );
    }
    Ok(())
}

/// A key is one or more dot-separated bare-key segments: `homepage`,
/// `window.width`.
pub fn validate_key(key: &str) -> Result<()> {
    if prefs_core::validate_key(key).is_err() {
        bail!(
            "invalid key `{key}`: expected dot-separated non-empty \
             [A-Za-z0-9_-] segments"
        );
    }
    Ok(())
}

/// An existing key in `ns` that cannot coexist with `key` — one is a strict
/// segment-wise prefix of the other, so TOML would need the same name to be
/// both a value and a table.
fn colliding_key<'a>(entries: &'a Namespace, key: &str) -> Option<&'a str> {
    entries
        .keys()
        .find(|existing| existing.as_str() != key && is_prefix_path(existing, key))
        .map(|k| k.as_str())
}

fn is_prefix_path(a: &str, b: &str) -> bool {
    prefs_core::is_prefix_path(a, b)
}

/// The bus value as the shared core's value (lossless: the same four scalars).
pub fn to_core(v: &PrefValue) -> prefs_core::PrefValue {
    match v {
        PrefValue::Str(s) => prefs_core::PrefValue::Str(s.clone()),
        PrefValue::Int(i) => prefs_core::PrefValue::Int(*i),
        PrefValue::Float(f) => prefs_core::PrefValue::Float(*f),
        PrefValue::Bool(b) => prefs_core::PrefValue::Bool(*b),
    }
}

/// The shared core's value as the bus value.
pub fn from_core(v: &prefs_core::PrefValue) -> PrefValue {
    match v {
        prefs_core::PrefValue::Str(s) => PrefValue::Str(s.clone()),
        prefs_core::PrefValue::Int(i) => PrefValue::Int(*i),
        prefs_core::PrefValue::Float(f) => PrefValue::Float(*f),
        prefs_core::PrefValue::Bool(b) => PrefValue::Bool(*b),
    }
}

// ---------------------------------------------------------------------------
// TOML <-> PrefValue
// ---------------------------------------------------------------------------

fn to_toml_value(v: &PrefValue) -> toml::Value {
    match v {
        PrefValue::Str(s) => toml::Value::String(s.clone()),
        PrefValue::Int(i) => toml::Value::Integer(*i),
        PrefValue::Float(f) => toml::Value::Float(*f),
        PrefValue::Bool(b) => toml::Value::Boolean(*b),
    }
}

/// The four scalar types the store carries. Anything else in the file (array,
/// datetime) is outside the value domain and is skipped on load rather than
/// coerced into a lie.
fn from_toml_value(v: &toml::Value) -> Option<PrefValue> {
    match v {
        toml::Value::String(s) => Some(PrefValue::Str(s.clone())),
        toml::Value::Integer(i) => Some(PrefValue::Int(*i)),
        toml::Value::Float(f) => Some(PrefValue::Float(*f)),
        toml::Value::Boolean(b) => Some(PrefValue::Bool(*b)),
        _ => None,
    }
}

/// Expand `a.b.c = v` into nested tables, failing rather than clobbering if a
/// path segment is already a value (the collision `set` rejects up front).
fn insert_dotted(table: &mut toml::Table, key: &str, value: toml::Value) -> Result<()> {
    let mut segments = key.split('.').peekable();
    let mut cursor = table;
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            cursor.insert(segment.to_string(), value);
            return Ok(());
        }
        let next = cursor
            .entry(segment.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        cursor = match next {
            toml::Value::Table(t) => t,
            _ => bail!("path segment `{segment}` of `{key}` is already a value"),
        };
    }
    unreachable!("a validated key has at least one segment")
}

/// Flatten a namespace's (possibly nested) table back into dotted keys.
fn flatten_into(prefix: &str, table: &toml::Table, out: &mut Namespace) {
    for (name, value) in table {
        let key = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        match value {
            toml::Value::Table(inner) => flatten_into(&key, inner, out),
            other => {
                if let Some(v) = from_toml_value(other) {
                    out.insert(key, v);
                } else {
                    log::warn!(
                        "[PRINCIPIA] :: ignoring `{key}`: unsupported preference value type"
                    );
                }
            }
        }
    }
}

fn parse_document(text: &str) -> Result<BTreeMap<String, Namespace>> {
    let doc: toml::Table = toml::from_str(text)?;
    let mut namespaces = BTreeMap::new();
    for (ns, value) in &doc {
        match value {
            toml::Value::Table(table) => {
                let mut entries = Namespace::new();
                flatten_into("", table, &mut entries);
                if !entries.is_empty() {
                    namespaces.insert(ns.clone(), entries);
                }
            }
            _ => {
                log::warn!(
                    "[PRINCIPIA] :: ignoring top-level `{ns}`: preferences live under \
                     a [namespace] table"
                );
            }
        }
    }
    Ok(namespaces)
}

// ---------------------------------------------------------------------------
// TESTS
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &tempfile::TempDir) -> PrefStore {
        PrefStore::load(dir.path().join("preferences.toml")).expect("fresh store loads")
    }

    #[test]
    fn round_trips_every_value_type_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(&dir);

        s.set("aether", "homepage", PrefValue::Str("https://una.os/".into()))
            .unwrap();
        s.set("aether", "window.width", PrefValue::Int(1280)).unwrap();
        s.set("system", "scale", PrefValue::Float(1.5)).unwrap();
        s.set("system", "verbose", PrefValue::Bool(true)).unwrap();

        // The atomic write landed, and left no temp file behind.
        let path = dir.path().join("preferences.toml");
        assert!(path.exists(), "the store file must exist after a set");
        let strays: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "preferences.toml")
            .collect();
        assert!(strays.is_empty(), "temp files must be renamed away: {strays:?}");

        // A fresh load sees exactly what was set, with types intact.
        let reloaded = PrefStore::load(&path).unwrap();
        assert_eq!(
            reloaded.get("aether", "homepage"),
            Some(PrefValue::Str("https://una.os/".into()))
        );
        assert_eq!(reloaded.get("aether", "window.width"), Some(PrefValue::Int(1280)));
        assert_eq!(reloaded.get("system", "scale"), Some(PrefValue::Float(1.5)));
        assert_eq!(reloaded.get("system", "verbose"), Some(PrefValue::Bool(true)));
        assert_eq!(reloaded.get("aether", "nothing"), None, "unset answers None");
    }

    /// An integer-valued float must not come back as an Int: a consumer that
    /// asked for a scale factor would silently lose the type.
    #[test]
    fn a_whole_float_stays_a_float() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(&dir);
        s.set("system", "scale", PrefValue::Float(2.0)).unwrap();
        let reloaded = PrefStore::load(s.path()).unwrap();
        assert_eq!(reloaded.get("system", "scale"), Some(PrefValue::Float(2.0)));
    }

    #[test]
    fn namespaces_are_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(&dir);
        s.set("aether", "homepage", PrefValue::Str("a".into())).unwrap();
        s.set("stria", "homepage", PrefValue::Str("s".into())).unwrap();

        assert_eq!(s.get("aether", "homepage"), Some(PrefValue::Str("a".into())));
        assert_eq!(s.get("stria", "homepage"), Some(PrefValue::Str("s".into())));
        assert_eq!(s.get("vein", "homepage"), None);
        assert_eq!(s.list("vein"), vec![], "an unknown namespace lists empty");
        assert_eq!(s.namespaces(), vec!["aether".to_string(), "stria".to_string()]);
    }

    #[test]
    fn list_is_sorted_and_scoped_to_one_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(&dir);
        s.set("aether", "window.width", PrefValue::Int(2)).unwrap();
        s.set("aether", "homepage", PrefValue::Str("h".into())).unwrap();
        s.set("stria", "muted", PrefValue::Bool(false)).unwrap();

        assert_eq!(
            s.list("aether"),
            vec![
                ("homepage".to_string(), PrefValue::Str("h".into())),
                ("window.width".to_string(), PrefValue::Int(2)),
            ]
        );
        assert_eq!(s.list("stria"), vec![("muted".to_string(), PrefValue::Bool(false))]);
    }

    #[test]
    fn set_overwrites_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(&dir);
        s.set("aether", "window.width", PrefValue::Int(800)).unwrap();
        s.set("aether", "window.width", PrefValue::Int(1280)).unwrap();
        assert_eq!(s.list("aether").len(), 1);
        assert_eq!(
            PrefStore::load(s.path()).unwrap().get("aether", "window.width"),
            Some(PrefValue::Int(1280))
        );
    }

    #[test]
    fn malformed_namespaces_and_keys_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(&dir);
        assert!(s.set("", "k", PrefValue::Int(1)).is_err());
        assert!(s.set("ae ther", "k", PrefValue::Int(1)).is_err());
        assert!(s.set("aether", "", PrefValue::Int(1)).is_err());
        assert!(s.set("aether", "window..width", PrefValue::Int(1)).is_err());
        assert!(s.set("aether", ".width", PrefValue::Int(1)).is_err());
        assert!(!s.path().exists(), "a rejected set must not create the file");
    }

    #[test]
    fn a_key_cannot_be_both_a_value_and_a_table() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(&dir);
        s.set("aether", "window", PrefValue::Int(1)).unwrap();
        assert!(s.set("aether", "window.width", PrefValue::Int(2)).is_err());
        // ...and the rejected key left no trace in the cache.
        assert_eq!(s.list("aether"), vec![("window".to_string(), PrefValue::Int(1))]);

        let mut s2 = store(&tempfile::tempdir().unwrap());
        s2.set("aether", "window.width", PrefValue::Int(2)).unwrap();
        assert!(s2.set("aether", "window", PrefValue::Int(1)).is_err());
    }

    /// PRINCIPIA2 (SR32): every declared key clamps exactly as the shared core
    /// says, at the store — the in-process writer cannot bypass it.
    #[test]
    fn the_store_clamps_every_declared_range_through_prefs_core() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(&dir);
        for k in prefs_core::schema::SCHEMA {
            if let prefs_core::schema::Kind::Int { min, max } = k.kind {
                for x in [min.saturating_sub(1), min, max, max.saturating_add(1)] {
                    let want = prefs_core::schema::check(k.ns, k.key, prefs_core::PrefValue::Int(x)).unwrap();
                    let got = s.set(k.ns, k.key, PrefValue::Int(x)).unwrap();
                    assert_eq!((to_core(&got.value), got.clamped), (want.value, want.clamped), "{}.{} = {x}", k.ns, k.key);
                    assert_eq!(s.get(k.ns, k.key), Some(got.value));
                }
            }
        }
        let t = s.set("vein", "temperature", PrefValue::Float(9.0)).unwrap();
        assert_eq!((t.value, t.clamped), (PrefValue::Float(2.0), true));
        // Undeclared keys are the app's own business: stored as given.
        let a = s.set("aether", "window.width", PrefValue::Int(-5)).unwrap();
        assert_eq!((a.value, a.clamped), (PrefValue::Int(-5), false));
        assert!(s.set("vein", "provider", PrefValue::Str("hal9000".into())).is_err());
        assert_eq!(s.get("vein", "provider"), None, "a refused write leaves no trace");
    }

    /// PRINCIPIA2 M3 (R81): `vein.embed.provider`'s derived default, every
    /// branch, on the host — the real model cache on disk, a scripted env.
    #[test]
    fn the_embedder_rule_on_the_host() {
        use prefs_core::rules::Rule;
        use prefs_core::schema::Source;
        let dir = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let mut s = store(&dir);
        let xdg = cache.path().to_string_lossy().to_string();
        let env_with = |vars: &'static [(&'static str, &'static str)], xdg: String| {
            move |k: &str| {
                if k == "XDG_CACHE_HOME" {
                    return Some(xdg.clone());
                }
                vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
            }
        };
        let rule = Source::Rule(Rule::Embedder);
        let s_ = |x: &str| PrefValue::Str(x.into());

        // off: no key variable, no model.
        assert_eq!(s.effective("vein", "embed.provider", env_with(&[], xdg.clone())), Some((s_("off"), rule)));
        // a blank key variable is not a key.
        assert_eq!(
            s.effective("vein", "embed.provider", env_with(&[("GEMINI_API_KEY", " ")], xdg.clone())).unwrap().0,
            s_("off")
        );
        // local: install the manifest's files (a partial install is not installed).
        let mdir = models_dir(&env_with(&[], xdg.clone())).join(prefs_core::schema::LOCAL_EMBED_MODEL);
        fs::create_dir_all(&mdir).unwrap();
        fs::write(mdir.join("model.onnx"), b"x").unwrap();
        assert_eq!(s.effective("vein", "embed.provider", env_with(&[], xdg.clone())).unwrap().0, s_("off"));
        fs::write(mdir.join("vocab.txt"), b"x").unwrap();
        fs::write(mdir.join("config.json"), b"x").unwrap();
        assert_eq!(s.effective("vein", "embed.provider", env_with(&[], xdg.clone())), Some((s_("local"), rule)));
        // gemini: the default key variable is set — wins over the installed model.
        assert_eq!(
            s.effective("vein", "embed.provider", env_with(&[("GEMINI_API_KEY", "k")], xdg.clone())),
            Some((s_("gemini"), rule))
        );
        // ...and follows `vein.gemini.api_key_env` when it names another variable.
        s.set("vein", "gemini.api_key_env", s_("MY_GEM")).unwrap();
        assert_eq!(
            s.effective("vein", "embed.provider", env_with(&[("GEMINI_API_KEY", "k")], xdg.clone())).unwrap().0,
            s_("local")
        );
        assert_eq!(
            s.effective("vein", "embed.provider", env_with(&[("MY_GEM", "k")], xdg.clone())).unwrap().0,
            s_("gemini")
        );
        // A stored choice beats the rule; the store itself never holds a default.
        s.set("vein", "embed.provider", s_("off")).unwrap();
        assert_eq!(
            s.effective("vein", "embed.provider", env_with(&[("MY_GEM", "k")], xdg.clone())),
            Some((s_("off"), Source::Stored))
        );
        assert_eq!(s.get("vein", "provider"), None);
        // vein.provider defaults to claude; the chat model follows it.
        assert_eq!(s.effective("vein", "provider", env_with(&[], xdg.clone())), Some((s_("claude"), Source::Default)));
        assert_eq!(
            s.effective("vein", "model", env_with(&[], xdg.clone())),
            Some((s_("claude-opus-5-5"), Source::Rule(Rule::ChatModel)))
        );
    }

    #[test]
    fn the_file_is_hand_editable_toml() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = store(&dir);
        s.set("aether", "window.width", PrefValue::Int(1280)).unwrap();
        let text = fs::read_to_string(s.path()).unwrap();
        assert!(text.contains("[aether.window]"), "dotted keys nest: {text}");
        assert!(text.contains("width = 1280"), "{text}");
    }

    /// A file written by hand (or by a future version) with a value type the
    /// store does not carry must not poison the load.
    #[test]
    fn unsupported_value_types_are_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.toml");
        fs::write(
            &path,
            "[aether]\nhomepage = \"x\"\nrecents = [\"a\", \"b\"]\n",
        )
        .unwrap();
        let s = PrefStore::load(&path).unwrap();
        assert_eq!(s.get("aether", "homepage"), Some(PrefValue::Str("x".into())));
        assert_eq!(s.get("aether", "recents"), None);
    }

    #[test]
    fn a_malformed_file_is_an_error_not_a_silent_wipe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.toml");
        fs::write(&path, "this is not = = toml").unwrap();
        assert!(PrefStore::load(&path).is_err());
    }

    /// PREFS (B300): the tree `prefs_core`'s golden test uses, built through THIS store.
    fn golden_store(dir: &tempfile::TempDir) -> PrefStore {
        let mut s = store(dir);
        s.set("aether", "homepage", PrefValue::Str("https://una.os/".into())).unwrap();
        s.set("aether", "window.width", PrefValue::Int(1280)).unwrap();
        s.set("aether", "window.height", PrefValue::Int(-800)).unwrap();
        s.set("aether", "window.deep.on", PrefValue::Bool(true)).unwrap();
        s.set("aether", "quote", PrefValue::Str("say \"hi\" \\ there".into())).unwrap();
        s.set("aether", "lines", PrefValue::Str("a\nb".into())).unwrap();
        s.set("aether", "ctl", PrefValue::Str("a\tb\u{7}".into())).unwrap();
        s.set("aether", "apos", PrefValue::Str("it's".into())).unwrap();
        s.set("aether", "uni", PrefValue::Str("héllo — ok".into())).unwrap();
        s.set("system", "display.brightness", PrefValue::Int(12)).unwrap();
        s.set("system", "display.scale", PrefValue::Float(2.0)).unwrap();
        s.set("system", "display.gamma", PrefValue::Float(1.5e-7)).unwrap();
        s.set("system", "display.big", PrefValue::Float(1e20)).unwrap();
        s.set("system", "audio.mute", PrefValue::Bool(false)).unwrap();
        s.set("system", "dock.pins", PrefValue::Str("console,shell,quarry".into())).unwrap();
        s
    }

    /// PREFS (B300): every file this store writes is inside the kernel's subset — `prefs_core` parses it
    /// to the same values and re-emits it byte for byte; and the golden `prefs_core` tests against is
    /// exactly what this store writes today (a `toml` upgrade that changed the layout fails HERE).
    #[test]
    fn prefs_core_accepts_every_to_toml_output() {
        let dir = tempfile::tempdir().unwrap();
        let s = golden_store(&dir);
        let text = s.to_toml().unwrap();
        assert_eq!(
            text,
            include_str!("../../../unaos/libs/sys/prefs_core/tests/principia_golden.toml"),
            "prefs_core's golden is no longer what principia writes"
        );
        let tree = prefs_core::PrefTree::parse(&text).expect("prefs_core accepts principia's output");
        assert_eq!(tree.to_toml(), text, "byte-identical re-emit");
        for ns in s.namespaces() {
            let mine: Vec<_> = s.list(&ns).into_iter().map(|(k, v)| (k, to_core(&v))).collect();
            let core: Vec<_> = tree.list(&ns).into_iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
            assert_eq!(mine, core, "namespace {ns}");
        }
        // A single-key file (the shape a fresh kernel store writes) too.
        let dir2 = tempfile::tempdir().unwrap();
        let mut one = store(&dir2);
        one.set("system", "display.idle_min", PrefValue::Int(10)).unwrap();
        let t1 = one.to_toml().unwrap();
        assert_eq!(prefs_core::PrefTree::parse(&t1).unwrap().to_toml(), t1);
    }

    /// PREFS (B300): a file the KERNEL wrote (prefs_core's emitter) loads here to the same values.
    #[test]
    fn a_prefs_core_file_loads_in_principia() {
        let mut t = prefs_core::PrefTree::new();
        t.set("system", "display.brightness", prefs_core::PrefValue::Int(9)).unwrap();
        t.set("system", "display.wallpaper", prefs_core::PrefValue::Str("/home/ann/SKY.PNG".into())).unwrap();
        t.set("system", "audio.mute", prefs_core::PrefValue::Bool(true)).unwrap();
        t.set("system", "dock.pins", prefs_core::PrefValue::Str("console,shell".into())).unwrap();
        t.set("system", "pointer.speed", prefs_core::PrefValue::Int(2)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.toml");
        fs::write(&path, t.to_toml()).unwrap();
        let s = PrefStore::load(&path).unwrap();
        for (ns, key, v) in t.entries() {
            assert_eq!(s.get(ns, key).map(|v| to_core(&v)), Some(v.clone()), "{ns}.{key}");
        }
        assert_eq!(s.to_toml().unwrap(), t.to_toml(), "and principia re-writes it byte for byte");
    }
}
