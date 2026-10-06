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

//! The preference store: namespaced, typed, TOML-backed, atomically written —
//! ONE store with the kernel's (PRINCIPIAFILES, rmbp-ledger B445; R79, R98).
//!
//! # Shape
//!
//! A preference is addressed by a **namespace** (a per-app/domain string —
//! `"aether"`, `"stria"`, `"system"`) and a **dotted key** within it
//! (`"homepage"`, `"window.width"`). The value is one of four scalar types
//! ([`PrefValue`]) — exactly what a TOML scalar carries losslessly, so nothing
//! is retyped by a save/load cycle.
//!
//! On disk the store is a FOLDER, `<home>/settings/`, one human-readable TOML
//! file per DOMAIN (`display`, `login`, `desktop`, `sound`, `trackpad`,
//! `general`, and a program's own: `vein`, `aether`, `app.<name>.*` -> `<name>`).
//! Everything about that shape is `prefs_core::files` — the domain rule, the
//! split, the text (`# auto-saved <ISO> by <who>`, the schema's doc above each
//! key) — and every file is read with `prefs_core::PrefTree::parse`: the very
//! code the kernel (`unaos/crates/kernel/src/prefs.rs`) reads and writes the
//! same folder with. The single `~/.config/unaos/preferences.toml` of before
//! R98 is migrated ONCE into the folder and deleted, by the writer of record
//! ([`PrefStore::settle`], which `Principia` runs), exactly as the kernel does.
//!
//! # Rules
//!
//! - **Defaults live with the consumer.** The store never invents a value;
//!   [`PrefStore::get`] answers `Option`, and an unset key is simply unset.
//! - **Every write is atomic, and touches one domain.** A set renders its
//!   domain's file into `<domain>.new` (the kernel's swap name), fsyncs it,
//!   reads it back (it must parse to the domain's tree), and `rename`s it over
//!   the real one — a reader (or a crash) sees the old file or the new one.
//! - **A refused file is never overwritten.** A domain file outside the subset
//!   is not adopted; its writes are HELD (the kernel's rule), so the user's
//!   file survives until they fix or delete it.
//! - **A key is a leaf.** `window` and `window.width` cannot both hold values,
//!   because TOML cannot express it; the collision is rejected at set time
//!   rather than at save time, so the in-memory state never diverges from what
//!   is persistable.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use bandy::PrefValue;
use prefs_core::{PrefTree, files};

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

/// The namespaced preference store, cached in memory and backed by the
/// `<home>/settings/` folder of domain files.
///
/// Reload-on-external-change is **not** implemented: the store is the writer of
/// record, and an edit made to a file underneath a running Principia is not
/// noticed until the next load. (Queued — see the README.)
pub struct PrefStore {
    dir: PathBuf,
    namespaces: BTreeMap<String, Namespace>,
    /// Domains whose file `prefs_core` refused at load: their writes are held.
    held: BTreeSet<String>,
    /// The tree came from the pre-R98 single file: [`PrefStore::settle`] writes the folder and deletes it.
    migrate_from: Option<PathBuf>,
    /// Values the load clamped into their schema range (re-saved by [`PrefStore::settle`]).
    clamped: usize,
    /// The `<iso>` of the auto-saved line (the UTC clock; a fixed one in tests).
    clock: fn() -> String,
}

impl PrefStore {
    /// Load the store from the folder `dir` (`<home>/settings`). READ-ONLY: a
    /// reader (Vein, Quartzite) may call it. A missing folder is an empty
    /// store — first boot has no preferences. Each `<domain>` file (or, when it
    /// is absent, an orphaned `<domain>.new`: its swap was interrupted) is
    /// parsed by `prefs_core`; a refused one is NOT adopted and its domain is
    /// held. With no domain file at all, the pre-R98 single file
    /// (`<dir>/../` + [`files::LEGACY`]) is read in its place, and
    /// [`PrefStore::settle`] migrates it. Values outside their schema range are
    /// clamped by the shared rule, as the kernel's load does.
    pub fn load(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        let mut tree = PrefTree::new();
        let (mut held, mut found) = (BTreeSet::new(), false);
        for d in domains_in(&dir)? {
            match read_domain(&dir, &d) {
                Ok(Some(t)) => {
                    files::merge(&mut tree, &t);
                    found = true;
                }
                Ok(None) => {}
                Err(e) => {
                    log::error!("[PRINCIPIA] :: settings/{d} refused ({e:#}); its writes are held — fix or delete it");
                    held.insert(d);
                }
            }
        }
        let mut migrate_from = None;
        if !found && held.is_empty() {
            let lp = legacy_of(&dir);
            match fs::read_to_string(&lp) {
                Ok(text) => match PrefTree::parse(&text) {
                    Ok(t) => {
                        tree = t;
                        migrate_from = Some(lp);
                    }
                    Err(e) => log::error!(
                        "[PRINCIPIA] :: {} refused (line {}: {}): not migrated, left in place (R98)",
                        lp.display(), e.line, e.why
                    ),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("reading {}", lp.display())),
            }
        }
        let clamped = prefs_core::schema::clamp_tree(&mut tree);
        let mut s = Self::empty(dir);
        s.held = held;
        s.migrate_from = migrate_from;
        s.clamped = clamped;
        for (ns, k, v) in tree.entries() {
            s.namespaces.entry(ns.to_string()).or_default().insert(k.to_string(), from_core(v));
        }
        Ok(s)
    }

    /// An empty store bound to the folder `dir`, without reading anything.
    pub fn empty(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            namespaces: BTreeMap::new(),
            held: BTreeSet::new(),
            migrate_from: None,
            clamped: 0,
            clock: now_iso,
        }
    }

    /// The writer of record's step after [`PrefStore::load`] (the kernel's
    /// load does the same): a tree read from the pre-R98 single file is
    /// written as the domain files and the old file (and its `.new`) is
    /// DELETED; a load that clamped re-saves, so the files hold the clamp.
    /// Answers whether the legacy file was migrated.
    pub fn settle(&mut self) -> Result<bool> {
        if let Some(lp) = self.migrate_from.clone() {
            let n = self.save_all()?;
            let _ = fs::remove_file(&lp);
            let mut tmp = lp.clone().into_os_string();
            tmp.push(".new");
            let _ = fs::remove_file(PathBuf::from(tmp));
            self.migrate_from = None;
            self.clamped = 0;
            log::info!("[PRINCIPIA] :: migrated {} -> {} ({n} keys, R98)", lp.display(), self.dir.display());
            return Ok(true);
        }
        if self.clamped > 0 {
            self.save_all()?;
            self.clamped = 0;
        }
        Ok(false)
    }

    /// Replace the auto-saved line's clock (tests pin it to compare bytes).
    pub fn set_clock(&mut self, clock: fn() -> String) {
        self.clock = clock;
    }

    /// The folder this store persists to (`<home>/settings`).
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// `<dir>/<domain>`.
    pub fn domain_path(&self, domain: &str) -> PathBuf {
        self.dir.join(domain)
    }

    /// Domains whose file was refused at load (their writes are held).
    pub fn held(&self) -> Vec<String> {
        self.held.iter().cloned().collect()
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

    /// Would setting `key` in `ns` collide with an existing dotted path?
    pub fn is_collision(&self, ns: &str, key: &str) -> bool {
        self.namespaces.get(ns).is_some_and(|n| colliding_key(n, key).is_some())
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

    /// Set `ns`/`key` and persist its DOMAIN's file atomically.
    ///
    /// The write first goes through the ONE schema validator both rings run,
    /// `prefs_core::schema::check` (PRINCIPIA2, SR32): a declared key written
    /// out of range is CLAMPED (the answer says so: [`SetOutcome::clamped`]);
    /// a wrong type, a value outside the key's enum, an over-long or
    /// unprintable string is refused. An undeclared key is stored as given.
    ///
    /// Errors on a malformed namespace/key, a schema refusal, a key that
    /// collides with an existing dotted path, a held domain, or a failed write
    /// — and on error the in-memory cache is left exactly as it was, so cache
    /// and files never disagree.
    pub fn set(&mut self, ns: &str, key: &str, value: PrefValue) -> Result<SetOutcome> {
        validate_ns(ns)?;
        validate_key(key)?;
        let applied = prefs_core::schema::check(ns, key, to_core(&value))
            .map_err(|r| anyhow::anyhow!("refused `{ns}`.`{key}` = {}: {r}", to_core(&value)))?;
        let outcome = SetOutcome { value: from_core(&applied.value), clamped: applied.clamped };
        let value = outcome.value.clone();
        let domain = files::domain_of(ns, key);
        if !files::valid_domain(&domain) {
            bail!("`{ns}`.`{key}` has no settings file (domain `{domain}`)");
        }
        if self.held.contains(&domain) {
            bail!("held: settings/{domain} was refused at load; fix or delete it");
        }

        let entry = self.namespaces.entry(ns.to_string()).or_default();
        if let Some(other) = colliding_key(entry, key) {
            bail!(
                "key `{key}` collides with `{other}` in namespace `{ns}`: \
                 one cannot be both a value and a table"
            );
        }

        let previous = entry.insert(key.to_string(), value);
        if let Err(e) = self.save_domain(&domain) {
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

    /// The whole store as `prefs_core`'s tree (the value model both rings share).
    pub fn tree(&self) -> PrefTree {
        let mut t = PrefTree::new();
        for (ns, entries) in &self.namespaces {
            for (k, v) in entries {
                let _ = t.set(ns, k, to_core(v));
            }
        }
        t
    }

    /// The text of `domain`'s file as it would be written now.
    pub fn render_domain(&self, domain: &str) -> String {
        let part = files::part(&self.tree(), domain);
        files::render(domain, &part, &(self.clock)(), &files::writer_of(domain), &doc_of)
    }

    /// Write EVERY domain the store holds (the migration, the load-time clamp).
    /// `Ok(keys written)`; a held domain is skipped.
    pub fn save_all(&self) -> Result<usize> {
        let mut n = 0;
        for d in files::split(&self.tree()).into_keys() {
            if !self.held.contains(&d) {
                n += self.save_domain(&d)?;
            }
        }
        Ok(n)
    }

    /// Write ONE domain's file by the kernel's swap: `<d>.new` written and
    /// fsynced, READ BACK and parsed (it must be the domain's tree), then
    /// renamed over `<d>`. `Ok(keys written)`.
    fn save_domain(&self, domain: &str) -> Result<usize> {
        let part = files::part(&self.tree(), domain);
        let text = files::render(domain, &part, &(self.clock)(), &files::writer_of(domain), &doc_of);
        fs::create_dir_all(&self.dir).with_context(|| format!("creating {}", self.dir.display()))?;
        let path = self.domain_path(domain);
        let tmp = self.dir.join(format!("{domain}.new"));
        {
            let mut f = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            f.write_all(text.as_bytes()).with_context(|| format!("writing {}", tmp.display()))?;
            f.sync_all().with_context(|| format!("syncing {}", tmp.display()))?;
        }
        let back = fs::read_to_string(&tmp).with_context(|| format!("reading back {}", tmp.display()))?;
        match PrefTree::parse(&back) {
            Ok(t) if back == text && (t == part || t.to_toml() == part.to_toml()) => {}
            _ => bail!("read-back of {} is not the domain's tree", tmp.display()),
        }
        fs::rename(&tmp, &path)
            .with_context(|| format!("renaming {} onto {}", tmp.display(), path.display()))?;
        Ok(part.len())
    }

    /// The whole store as ONE TOML document (`prefs_core`'s emission of the
    /// tree) — the pre-R98 single file's shape; the files are per domain.
    pub fn to_toml(&self) -> Result<String> {
        Ok(self.tree().to_toml())
    }
}

/// The comment above a key: the schema's `doc`. A program's declared doc
/// (`PrefDeclare`, `app.<name>.*`) is the kernel's registry; Principia has
/// none yet, so an `app` key is written without one.
fn doc_of(ns: &str, key: &str) -> Option<String> {
    if ns == files::APP_NS {
        return None;
    }
    files::schema_doc(ns, key)
}

/// `<dir>/..` + [`files::LEGACY`]: the pre-R98 single file of the home that
/// holds `dir` (the kernel's `legacy_path`).
pub fn legacy_of(dir: &Path) -> PathBuf {
    dir.parent().unwrap_or(Path::new("")).join(files::LEGACY)
}

/// The domains in `dir` (a `<d>.new` alone counts: its swap was interrupted).
fn domains_in(dir: &Path) -> Result<Vec<String>> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("listing {}", dir.display())),
    };
    let mut v = BTreeSet::new();
    for e in rd.flatten() {
        if !e.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let d = name.strip_suffix(".new").unwrap_or(&name);
        if files::valid_domain(d) {
            v.insert(d.to_string());
        }
    }
    Ok(v.into_iter().collect())
}

/// One domain file (or its orphaned `.new`): `Ok(None)` = absent, `Err` = refused.
fn read_domain(dir: &Path, d: &str) -> Result<Option<PrefTree>> {
    let parse = |p: &Path, text: &str| {
        PrefTree::parse(text).map_err(|e| anyhow::anyhow!("{} line {}: {}", p.display(), e.line, e.why))
    };
    let p = dir.join(d);
    match fs::read_to_string(&p) {
        Ok(t) => return parse(&p, &t).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("reading {}", p.display())),
    }
    let tmp = dir.join(format!("{d}.new"));
    match fs::read_to_string(&tmp).ok().map(|t| parse(&tmp, &t)) {
        Some(Ok(t)) => {
            log::info!("[PRINCIPIA] :: adopted {} (the swap was interrupted)", tmp.display());
            Ok(Some(t))
        }
        _ => Ok(None),
    }
}

/// UTC now as `YYYY-MM-DDTHH:MM:SSZ` (the kernel's `clock::iso8601_now` shape).
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    iso_of(secs)
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ` (civil-from-days, proleptic Gregorian).
pub fn iso_of(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

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
// TESTS
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &tempfile::TempDir) -> PrefStore {
        PrefStore::load(dir.path().join(files::DIR)).expect("fresh store loads")
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

        // The atomic writes landed, one file per domain, and left no temp file behind.
        let path = s.path().to_path_buf();
        let mut names: Vec<_> = fs::read_dir(&path)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(names, ["aether", "general"], "one file per domain, temp files renamed away");

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
        let text = fs::read_to_string(s.domain_path("aether")).unwrap();
        assert!(text.starts_with("# settings/aether"), "{text}");
        assert!(text.contains("# auto-saved ") && text.contains(" by the program aether\n"), "{text}");
        assert!(text.contains("\n[aether]\nwindow.width = 1280\n"), "dotted keys on their own line: {text}");
    }

    /// A domain file written by hand with a value type outside the subset is
    /// REFUSED as the kernel refuses it: its domain is held (never overwritten),
    /// every other domain loads.
    #[test]
    fn a_refused_domain_is_held_not_wiped() {
        let dir = tempfile::tempdir().unwrap();
        let sd = dir.path().join(files::DIR);
        fs::create_dir_all(&sd).unwrap();
        let bad = "[aether]\nhomepage = \"x\"\nrecents = [\"a\", \"b\"]\n";
        fs::write(sd.join("aether"), bad).unwrap();
        fs::write(sd.join("sound"), "[system]\naudio.volume = 7\n").unwrap();
        let mut s = PrefStore::load(&sd).unwrap();
        assert_eq!(s.held(), ["aether"]);
        assert_eq!(s.get("aether", "homepage"), None);
        assert_eq!(s.get("system", "audio.volume"), Some(PrefValue::Int(7)));
        assert!(s.set("aether", "homepage", PrefValue::Str("y".into())).is_err(), "held");
        assert_eq!(fs::read_to_string(sd.join("aether")).unwrap(), bad, "the user's file is untouched");
        s.set("system", "audio.volume", PrefValue::Int(9)).unwrap();
        assert!(!s.settle().unwrap());
        assert_eq!(fs::read_to_string(sd.join("aether")).unwrap(), bad);
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

    fn fixed_clock() -> String {
        String::from("2026-10-06T12:00:00Z")
    }

    /// The kernel's load (`unaos/crates/kernel/src/prefs.rs` `load`), minus the VFS: every domain file
    /// parsed by `prefs_core` and merged; then its `save_domain` text for each domain with the file's
    /// own stamp. What a kernel would write back after reading this folder.
    fn kernel_reads_and_rewrites(dir: &Path) -> BTreeMap<String, String> {
        let mut tree = PrefTree::new();
        let mut stamps = BTreeMap::new();
        for d in domains_in(dir).unwrap() {
            let text = fs::read_to_string(dir.join(&d)).unwrap();
            let (iso, by) = files::stamp_of(&text).expect("an auto-saved line");
            stamps.insert(d.clone(), (iso.to_string(), by.to_string()));
            files::merge(&mut tree, &PrefTree::parse(&text).expect("the kernel accepts principia's file"));
        }
        let kdoc = |ns: &str, k: &str| if ns == files::APP_NS { None } else { files::schema_doc(ns, k) };
        files::split(&tree)
            .iter()
            .map(|(d, part)| {
                let (iso, by) = &stamps[d];
                assert_eq!(by, &files::writer_of(d), "the kernel's by for {d}");
                (d.clone(), files::render(d, part, iso, by, &kdoc))
            })
            .collect()
    }

    fn folder_bytes(dir: &Path) -> BTreeMap<String, String> {
        domains_in(dir).unwrap().into_iter().map(|d| (d.clone(), fs::read_to_string(dir.join(&d)).unwrap())).collect()
    }

    /// PRINCIPIAFILES (B445): Principia writes the folder → the kernel's reader reads it and re-renders
    /// every domain BYTE FOR BYTE → Principia loads that and re-saves it byte for byte. One store.
    #[test]
    fn principia_kernel_principia_round_trip_is_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = golden_store(&dir);
        s.set_clock(fixed_clock);
        s.set("app", "lumen.window.frame", PrefValue::Str("10,20,800,600".into())).unwrap();
        s.set("vein", "temperature", PrefValue::Float(0.5)).unwrap();
        s.set("system", "login.items", PrefValue::Str("shell".into())).unwrap();
        s.set("system", "pointer.speed", PrefValue::Int(2)).unwrap();
        s.save_all().unwrap();
        let mine = folder_bytes(s.path());
        assert_eq!(
            mine.keys().map(String::as_str).collect::<Vec<_>>(),
            ["aether", "desktop", "display", "login", "lumen", "sound", "trackpad", "vein"]
        );
        let kern = kernel_reads_and_rewrites(s.path());
        assert_eq!(kern, mine, "the kernel re-renders principia's folder byte for byte");

        // ...and back: the kernel's bytes in a fresh home, loaded and re-saved by Principia.
        let home2 = tempfile::tempdir().unwrap();
        let d2 = home2.path().join(files::DIR);
        fs::create_dir_all(&d2).unwrap();
        for (d, text) in &kern {
            fs::write(d2.join(d), text).unwrap();
        }
        let mut back = PrefStore::load(&d2).unwrap();
        back.set_clock(fixed_clock);
        assert!(back.held().is_empty());
        assert_eq!(back.tree(), s.tree());
        back.save_all().unwrap();
        assert_eq!(folder_bytes(&d2), kern, "principia re-writes the kernel's folder byte for byte");
        for (d, text) in &kern {
            assert_eq!(&back.render_domain(d), text);
        }
    }

    /// PRINCIPIAFILES (B445), the migration both rings run: a pre-R98 `preferences.toml` (Principia's
    /// old single file; the kernel's emitter wrote the same bytes) becomes the domain files, once, and
    /// is deleted — by the writer of record. A reader (`load` alone) sees the values and writes nothing.
    #[test]
    fn the_legacy_single_file_migrates_once_and_is_deleted() {
        let mut t = PrefTree::new();
        t.set("system", "display.brightness", prefs_core::PrefValue::Int(9)).unwrap();
        t.set("system", "display.wallpaper", prefs_core::PrefValue::Str("/home/ann/SKY.PNG".into())).unwrap();
        t.set("system", "audio.mute", prefs_core::PrefValue::Bool(true)).unwrap();
        t.set("system", "dock.pins", prefs_core::PrefValue::Str("console,shell".into())).unwrap();
        t.set("system", "pointer.speed", prefs_core::PrefValue::Int(2)).unwrap();
        t.set("system", "audio.volume", prefs_core::PrefValue::Int(99)).unwrap(); // outside 0..16: clamped
        let home = tempfile::tempdir().unwrap();
        let sd = home.path().join(files::DIR);
        let lp = home.path().join(files::LEGACY);
        assert_eq!(legacy_of(&sd), lp);
        fs::create_dir_all(lp.parent().unwrap()).unwrap();
        fs::write(&lp, t.to_toml()).unwrap();

        let reader = PrefStore::load(&sd).unwrap();
        assert_eq!(reader.get("system", "display.brightness"), Some(PrefValue::Int(9)));
        assert_eq!(reader.get("system", "audio.volume"), Some(PrefValue::Int(16)), "the shared clamp");
        assert!(lp.exists() && !sd.exists(), "a reader migrates nothing");

        let mut w = PrefStore::load(&sd).unwrap();
        w.set_clock(fixed_clock);
        assert!(w.settle().unwrap());
        assert!(!lp.exists(), "the old file is deleted");
        assert_eq!(folder_bytes(&sd).keys().map(String::as_str).collect::<Vec<_>>(), ["desktop", "display", "sound", "trackpad"]);
        let again = PrefStore::load(&sd).unwrap();
        assert_eq!(again.tree(), w.tree());
        assert_eq!(again.get("system", "audio.volume"), Some(PrefValue::Int(16)));
        assert_eq!(kernel_reads_and_rewrites(&sd), folder_bytes(&sd));
        // A legacy file that reappears beside domain files is ignored (migrated ONCE).
        fs::write(&lp, "[system]\ndisplay.brightness = 3\n").unwrap();
        assert_eq!(PrefStore::load(&sd).unwrap().get("system", "display.brightness"), Some(PrefValue::Int(9)));
    }

    /// An interrupted swap (`<d>.new` alone) is adopted, as the kernel adopts it.
    #[test]
    fn an_orphaned_swap_is_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let sd = dir.path().join(files::DIR);
        fs::create_dir_all(&sd).unwrap();
        fs::write(sd.join("sound.new"), "[system]\naudio.volume = 5\n").unwrap();
        let s = PrefStore::load(&sd).unwrap();
        assert_eq!(s.get("system", "audio.volume"), Some(PrefValue::Int(5)));
    }

    #[test]
    fn iso_is_the_civil_utc_date() {
        assert_eq!(iso_of(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_of(1_791_288_000), "2026-10-06T12:00:00Z");
        assert_eq!(iso_of(951_782_400), "2000-02-29T00:00:00Z");
    }
}
