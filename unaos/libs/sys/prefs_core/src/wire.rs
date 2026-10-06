// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! THE PREFS WIRE — Principia's verbs as bytes, ONE implementation for both rings (PRINCIPIA2, SR32).
//!
//! The kernel fulfils `PREF_GET` / `PREF_SET` / `PREF_LIST` (una-abi 16/17/18) over the frozen v1 bus
//! frame (`docs/dev/evidence/rmbp-1001/PREFS.md`); Principia on the host answers the same verbs. Both call
//! [`fulfil`] over their own [`Store`], so a body in gives the same status and the same reply bytes on
//! either side. The frame header (magic, kind, corr, principal) is the transport's; this module owns the
//! BODIES.
//!
//! | verb | request body | reply body / status |
//! | :--- | :--- | :--- |
//! | GET 16 | `<ns>.<key>` | the value as a TOML literal; -ENOENT unset; -EINVAL malformed |
//! | SET 17 | `<ns>.<key>` NUL `<literal>` | empty when stored as sent; `<stored literal>` NUL `clamped=true` when the schema clamped it; -EACCES outside the session; -EINVAL malformed or refused by the schema; -EIO collision / save failed or held |
//! | LIST 18 | `<ns>` or empty (every namespace) | `<ns>.<key> = <literal>\n` lines, sorted; -E2BIG past [`BODY_MAX`] |
//!
//! The SET reply's clamp form is new with PRINCIPIA2: an unclamped SET still answers an EMPTY body, byte
//! for byte the kernel's B300 reply, so a caller that ignores the body is unchanged. PREFSKERNEL (B345):
//! the kernel's `prefs::bus_fulfil` IS [`fulfil`] over its store, and its `prefs::set` IS
//! [`persisted_set`] (name, [`crate::schema::check`], tree, save, rollback) — the reference [`TreeStore`]
//! runs the same function with a save that cannot fail.
//!
//! `PREF_CHANGED` (19): body = the SET request body of the stored value ([`changed_body`]); when the
//! schema clamped the write, `NUL clamped=true` follows ([`changed_body_applied`], [`parse_changed`]).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::schema::{self, Applied, Refusal};
use crate::{validate_key, validate_ns, PrefError, PrefTree, PrefValue};

/// una-abi `BUS_VERB_PREF_GET` (a dev-test pins the four to una-abi).
pub const VERB_GET: u8 = 16;
pub const VERB_SET: u8 = 17;
pub const VERB_LIST: u8 = 18;
pub const VERB_CHANGED: u8 = 19;
/// una-abi `BUS_BODY_MAX`: the reply ceiling.
pub const BODY_MAX: usize = 4096;

pub const ENOENT: i64 = -2;
pub const EIO: i64 = -5;
pub const E2BIG: i64 = -7;
pub const EACCES: i64 = -13;
pub const EINVAL: i64 = -22;

/// Why a [`Store::set`] failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetFail {
    /// The schema refused the value (-EINVAL).
    Refused(Refusal),
    /// A malformed name or a leaf collision (-EIO, the B300 status for a refused tree set).
    Name(PrefError),
    /// The store could not persist (-EIO).
    Io,
}

/// A preference store as the wire sees it. `set` MUST run [`schema::check`] (the kernel's and Principia's
/// do); [`TreeStore`] is the reference.
pub trait Store {
    fn get(&self, ns: &str, key: &str) -> Option<PrefValue>;
    fn set(&mut self, ns: &str, key: &str, v: PrefValue) -> Result<Applied, SetFail>;
    /// `(key, value)` in `ns`, sorted by key.
    fn list(&self, ns: &str) -> Vec<(String, PrefValue)>;
    /// Every namespace holding a value, sorted.
    fn namespaces(&self) -> Vec<String>;
}

/// `<ns>.<key>` (the bus's and the `pref` verb's address form), both halves validated.
pub fn split_addr(a: &str) -> Option<(&str, &str)> {
    let (ns, k) = a.split_once('.')?;
    (validate_ns(ns).is_ok() && validate_key(k).is_ok()).then_some((ns, k))
}

/// GET body. `None` = -EINVAL.
pub fn parse_get(body: &[u8]) -> Option<(&str, &str)> {
    split_addr(core::str::from_utf8(body).ok()?)
}

/// SET body: `<ns>.<key>` NUL `<TOML scalar literal>`. `None` = -EINVAL.
pub fn parse_set(body: &[u8]) -> Option<(&str, &str, PrefValue)> {
    let z = body.iter().position(|&b| b == 0)?;
    let (ns, k) = parse_get(&body[..z])?;
    let lit = core::str::from_utf8(&body[z + 1..]).ok()?;
    Some((ns, k, PrefValue::from_literal(lit).ok()?))
}

/// LIST body: `<ns>` or empty (every namespace). `None` = -EINVAL.
pub fn parse_list(body: &[u8]) -> Option<Option<&str>> {
    if body.is_empty() {
        return Some(None);
    }
    let ns = core::str::from_utf8(body).ok()?;
    validate_ns(ns).is_ok().then_some(Some(ns))
}

/// A SET request body (and the PREF_CHANGED body): `<ns>.<key>` NUL `<literal>`.
pub fn set_body(ns: &str, key: &str, v: &PrefValue) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(ns.as_bytes());
    b.push(b'.');
    b.extend_from_slice(key.as_bytes());
    b.push(0);
    b.extend_from_slice(v.to_literal().as_bytes());
    b
}

/// The PREF_CHANGED (19) body for a stored value.
pub fn changed_body(ns: &str, key: &str, stored: &PrefValue) -> Vec<u8> {
    set_body(ns, key, stored)
}

/// The PREF_CHANGED (19) body for an applied write: [`changed_body`], then `NUL clamped=true` when the
/// schema clamped it (PREFSKERNEL, B345 — the same suffix as the SET reply's clamp form).
pub fn changed_body_applied(ns: &str, key: &str, a: &Applied) -> Vec<u8> {
    let mut b = changed_body(ns, key, &a.value);
    if a.clamped {
        b.push(0);
        b.extend_from_slice(b"clamped=true");
    }
    b
}

/// Parse a PREF_CHANGED body: `(ns, key, stored value, clamped)`.
pub fn parse_changed(body: &[u8]) -> Option<(&str, &str, PrefValue, bool)> {
    let z = body.iter().position(|&b| b == 0)?;
    let (ns, k) = parse_get(&body[..z])?;
    let rest = &body[z + 1..];
    let (lit, clamped) = match rest.iter().position(|&b| b == 0) {
        Some(z2) if &rest[z2 + 1..] == b"clamped=true" => (&rest[..z2], true),
        Some(_) => return None,
        None => (rest, false),
    };
    let v = PrefValue::from_literal(core::str::from_utf8(lit).ok()?).ok()?;
    Some((ns, k, v, clamped))
}

/// The SET reply body: empty when stored as sent, else `<stored literal>` NUL `clamped=true`.
pub fn set_reply(a: &Applied, out: &mut Vec<u8>) {
    if a.clamped {
        out.extend_from_slice(a.value.to_literal().as_bytes());
        out.push(0);
        out.extend_from_slice(b"clamped=true");
    }
}

/// Parse a SET reply (the caller's side): `(stored value, clamped)`; `None` for an empty (unclamped)
/// reply — the value stored is the one sent.
pub fn parse_set_reply(body: &[u8]) -> Option<(PrefValue, bool)> {
    let z = body.iter().position(|&b| b == 0)?;
    let v = PrefValue::from_literal(core::str::from_utf8(&body[..z]).ok()?).ok()?;
    (&body[z + 1..] == b"clamped=true").then_some((v, true))
}

/// Fulfil one PREF verb over `store`. `in_session`: the caller runs in the open session (the transport
/// decides from the stamped principal). Reply body into `out`; returns the status (0 or a negative errno).
pub fn fulfil(store: &mut dyn Store, verb: u8, body: &[u8], in_session: bool, out: &mut Vec<u8>) -> i64 {
    match verb {
        VERB_GET => match parse_get(body) {
            Some((ns, k)) => match store.get(ns, k) {
                Some(v) => {
                    out.extend_from_slice(v.to_literal().as_bytes());
                    0
                }
                None => ENOENT,
            },
            None => EINVAL,
        },
        VERB_SET => match parse_set(body) {
            Some((ns, k, v)) => {
                if !in_session {
                    return EACCES;
                }
                match store.set(ns, k, v) {
                    Ok(a) => {
                        set_reply(&a, out);
                        0
                    }
                    Err(SetFail::Refused(_)) => EINVAL,
                    Err(SetFail::Name(_)) | Err(SetFail::Io) => EIO,
                }
            }
            None => EINVAL,
        },
        VERB_LIST => match parse_list(body) {
            Some(which) => {
                let nss = match which {
                    Some(ns) => alloc::vec![String::from(ns)],
                    None => store.namespaces(),
                };
                for ns in nss {
                    for (k, v) in store.list(&ns) {
                        out.extend_from_slice(format!("{}.{} = {}\n", ns, k, v).as_bytes());
                    }
                }
                if out.len() > BODY_MAX {
                    out.clear();
                    E2BIG
                } else {
                    0
                }
            }
            None => EINVAL,
        },
        _ => EINVAL,
    }
}

/// A store's tree and its persistence, as [`persisted_set`] sees them (PREFSKERNEL, B345). The kernel's
/// `with_tree` takes its `TREE` lock for the closure only (the save runs UNLOCKED, as it always has);
/// its `save` is the read-back swap of `preferences.toml`.
pub trait Persist {
    fn with_tree<R>(&mut self, f: impl FnOnce(&mut PrefTree) -> R) -> R;
    /// Persist the tree as it now is. `Err` = nothing durable changed (the caller rolls back).
    fn save(&mut self) -> Result<(), ()>;
}

/// What [`persisted_set`] did.
#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub applied: Applied,
    /// `false` = the stored value already was [`Applied::value`]: nothing saved, no PrefChanged.
    pub changed: bool,
}

/// THE store write, both rings' (PREFSKERNEL, B345): validate the name, run [`schema::check`] (the
/// clamp is what is stored), set the tree, save; a value equal to the stored one is not re-saved; a
/// failed save rolls the tree back, so cache and file never disagree (Principia's rule).
pub fn persisted_set<P: Persist>(p: &mut P, ns: &str, key: &str, v: PrefValue) -> Result<Stored, SetFail> {
    validate_ns(ns).map_err(SetFail::Name)?;
    validate_key(key).map_err(SetFail::Name)?;
    let applied = schema::check(ns, key, v).map_err(SetFail::Refused)?;
    let prev = p.with_tree(|t| t.set(ns, key, applied.value.clone())).map_err(SetFail::Name)?;
    if prev.as_ref() == Some(&applied.value) {
        return Ok(Stored { applied, changed: false });
    }
    if p.save().is_err() {
        p.with_tree(|t| match prev {
            Some(o) => {
                let _ = t.set(ns, key, o);
            }
            None => {
                t.remove(ns, key);
            }
        });
        return Err(SetFail::Io);
    }
    Ok(Stored { applied, changed: true })
}

/// An in-RAM tree persists trivially (the reference store's [`Persist`]).
impl Persist for PrefTree {
    fn with_tree<R>(&mut self, f: impl FnOnce(&mut PrefTree) -> R) -> R {
        f(self)
    }
    fn save(&mut self) -> Result<(), ()> {
        Ok(())
    }
}

/// The reference [`Store`]: an in-RAM [`PrefTree`] behind [`persisted_set`] — exactly the kernel's
/// `prefs::set` with a save that cannot fail.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeStore(pub PrefTree);

impl Store for TreeStore {
    fn get(&self, ns: &str, key: &str) -> Option<PrefValue> {
        self.0.get(ns, key).cloned()
    }
    fn set(&mut self, ns: &str, key: &str, v: PrefValue) -> Result<Applied, SetFail> {
        persisted_set(&mut self.0, ns, key, v).map(|s| s.applied)
    }
    fn list(&self, ns: &str) -> Vec<(String, PrefValue)> {
        self.0.list(ns).into_iter().map(|(k, v)| (String::from(k), v.clone())).collect()
    }
    fn namespaces(&self) -> Vec<String> {
        self.0.namespaces().into_iter().map(String::from).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{default_of, Source};

    fn run(s: &mut TreeStore, verb: u8, body: &[u8], in_session: bool) -> (i64, Vec<u8>) {
        let mut out = Vec::new();
        let st = fulfil(s, verb, body, in_session, &mut out);
        (st, out)
    }

    /// The kernel's PREFS-CODEC bodies (kernel `prefs::codec_selftest`, B300), unchanged.
    #[test]
    fn kat_the_b300_bodies() {
        // The body inside the frozen golden GET frame (`system.audio.mute`, corr 21).
        assert_eq!(parse_get(b"system.audio.mute"), Some(("system", "audio.mute")));
        assert_eq!(parse_set(b"system.display.brightness\x009"), Some(("system", "display.brightness", PrefValue::Int(9))));
        assert_eq!(
            parse_set(b"system.display.wallpaper\x00\"/home/ann/SKY.PNG\""),
            Some(("system", "display.wallpaper", PrefValue::Str("/home/ann/SKY.PNG".into())))
        );
        assert_eq!(parse_list(b""), Some(None));
        assert_eq!(parse_list(b"system"), Some(Some("system")));
        assert!(parse_get(b"system").is_none() && parse_get(b"system.a..b").is_none() && parse_get(b"sys tem.a").is_none());
        assert!(parse_set(b"system.a=1").is_none() && parse_set(b"system.a\x00[1]").is_none() && parse_set(b"system.a\x001 2").is_none());
        assert!(parse_list(b"a.b").is_none());
    }

    #[test]
    fn kat_set_unclamped_answers_the_b300_empty_body() {
        let mut s = TreeStore::default();
        assert_eq!(run(&mut s, VERB_SET, b"system.display.brightness\x009", true), (0, Vec::new()));
        assert_eq!(run(&mut s, VERB_GET, b"system.display.brightness", false), (0, b"9".to_vec()));
    }

    /// PRINCIPIA2: SET-with-clamp — frozen reply bytes.
    #[test]
    fn kat_set_with_clamp() {
        let mut s = TreeStore::default();
        let (st, body) = run(&mut s, VERB_SET, b"system.display.brightness\x000", true);
        assert_eq!((st, body.as_slice()), (0, b"1\x00clamped=true".as_slice()));
        assert_eq!(parse_set_reply(&body), Some((PrefValue::Int(1), true)));
        let (st, body) = run(&mut s, VERB_SET, b"system.audio.volume\x00-3", true);
        assert_eq!((st, body.as_slice()), (0, b"0\x00clamped=true".as_slice()));
        let (st, body) = run(&mut s, VERB_SET, b"vein.temperature\x007.5", true);
        assert_eq!((st, body.as_slice()), (0, b"2.0\x00clamped=true".as_slice()));
        assert_eq!(run(&mut s, VERB_GET, b"system.display.brightness", true), (0, b"1".to_vec()), "the clamp is stored");
        assert_eq!(parse_set_reply(b""), None);
    }

    #[test]
    fn kat_set_refusals_and_statuses() {
        let mut s = TreeStore::default();
        assert_eq!(run(&mut s, VERB_SET, b"system.audio.volume\x009", false), (EACCES, Vec::new()));
        assert_eq!(run(&mut s, VERB_SET, b"system.audio.volume\x00\"loud\"", true), (EINVAL, Vec::new()));
        assert_eq!(run(&mut s, VERB_SET, b"vein.provider\x00\"hal9000\"", true), (EINVAL, Vec::new()));
        assert_eq!(run(&mut s, VERB_SET, b"aether.window\x001", true), (0, Vec::new()));
        assert_eq!(run(&mut s, VERB_SET, b"aether.window.width\x002", true), (EIO, Vec::new()), "a leaf collision");
        assert_eq!(run(&mut s, VERB_GET, b"system.audio.volume", true), (ENOENT, Vec::new()));
        assert_eq!(run(&mut s, 99, b"", true), (EINVAL, Vec::new()));
    }

    /// PRINCIPIA2: the schema's `List` of a namespace — every `system` row with a static default set to
    /// it, listed: frozen bytes (the order is the tree's, i.e. the schema's).
    #[test]
    fn kat_list_of_the_system_namespace() {
        let mut s = TreeStore::default();
        let env = crate::rules::tests::Env::default();
        for k in schema::namespace("system") {
            if let Some((v, Source::Default)) = default_of(k.ns, k.key, &env) {
                assert_eq!(run(&mut s, VERB_SET, &set_body(k.ns, k.key, &v), true), (0, Vec::new()), "{}", k.key);
            }
        }
        const GOLDEN: &[u8] = b"system.audio.mute = false\n\
system.audio.volume = 12\n\
system.display.brightness = 12\n\
system.display.font = \"sans\"\n\
system.display.font_size = 13\n\
system.display.idle_min = 10\n\
system.display.wallpaper = \"\"\n\
system.login.items = \"\"\n\
system.pointer.speed = 1\n\
system.settings.tab = 0\n";
        assert_eq!(run(&mut s, VERB_LIST, b"system", false), (0, GOLDEN.to_vec()));
        // An empty LIST body lists every namespace; another namespace lists only itself.
        run(&mut s, VERB_SET, b"vein.provider\x00\"claude\"", true);
        let (st, all) = run(&mut s, VERB_LIST, b"", false);
        assert_eq!(st, 0);
        assert!(all.starts_with(GOLDEN) && all.ends_with(b"vein.provider = \"claude\"\n"));
        assert_eq!(run(&mut s, VERB_LIST, b"vein", false), (0, b"vein.provider = \"claude\"\n".to_vec()));
        assert_eq!(run(&mut s, VERB_LIST, b"nobody", false), (0, Vec::new()));
    }

    #[test]
    fn kat_list_past_the_ceiling_is_e2big() {
        let mut s = TreeStore::default();
        for i in 0..200 {
            s.set("aether", &format!("k{i:03}"), PrefValue::Str("x".repeat(20))).unwrap();
        }
        assert_eq!(run(&mut s, VERB_LIST, b"aether", false), (E2BIG, Vec::new()));
    }

    /// PREFSKERNEL: the PrefChanged body carries the clamp.
    #[test]
    fn kat_changed_body_applied() {
        let c = Applied { value: PrefValue::Int(1), clamped: true };
        assert_eq!(changed_body_applied("system", "display.brightness", &c), b"system.display.brightness\x001\x00clamped=true".to_vec());
        let u = Applied { value: PrefValue::Int(9), clamped: false };
        assert_eq!(changed_body_applied("system", "display.brightness", &u), changed_body("system", "display.brightness", &PrefValue::Int(9)));
        assert_eq!(parse_changed(b"system.display.brightness\x001\x00clamped=true"), Some(("system", "display.brightness", PrefValue::Int(1), true)));
        assert_eq!(parse_changed(b"system.display.brightness\x009"), Some(("system", "display.brightness", PrefValue::Int(9), false)));
        assert_eq!(parse_changed(b"system.display.brightness\x009\x00bogus"), None);
    }

    #[test]
    fn kat_changed_body() {
        assert_eq!(changed_body("system", "display.brightness", &PrefValue::Int(1)), b"system.display.brightness\x001".to_vec());
        assert_eq!(parse_set(&changed_body("vein", "provider", &PrefValue::Str("claude".into()))).unwrap().2, PrefValue::Str("claude".into()));
    }
}
