//! Import maps (HTML §8.1.5.5 "import maps" and §8.1.5.3 "resolve a module specifier", 2024 text with
//! multiple import maps merged): `<script type="importmap">` registers a map; every module specifier —
//! static `import`, dynamic `import()`, `import.meta.resolve` — resolves through it.
//!
//! CHARTER: Aether — binding (the HTML host half of module loading; js_core asks through `Host`).
//!
//! The page's map and its "resolved module set" are thread-local page state like the rest of `js::PAGE`,
//! reset with each new document ([`reset`] from `Engine::with_viewport`).

use html_core::url::{self, Url};
use std::cell::RefCell;

/// A specifier map: (normalized key, address or None for a blocked / invalid entry), sorted by key in
/// descending code-unit order (so a longer key that has a shorter one as prefix comes first).
type SpecMap = Vec<(String, Option<String>)>;

#[derive(Default, Clone, Debug)]
pub struct ImportMap {
    pub imports: SpecMap,
    /// (scope prefix URL, its specifier map), scopes in descending order.
    pub scopes: Vec<(String, SpecMap)>,
    /// (URL, integrity metadata).
    pub integrity: Vec<(String, String)>,
}

thread_local! {
    static MAP: RefCell<ImportMap> = RefCell::new(ImportMap::default());
    /// The global's resolved module set: (serialized base URL, normalized specifier, as-URL).
    static RESOLVED: RefCell<Vec<(String, String, Option<String>)>> = const { RefCell::new(Vec::new()) };
}

pub fn reset() {
    MAP.with(|m| *m.borrow_mut() = ImportMap::default());
    RESOLVED.with(|r| r.borrow_mut().clear());
}

/// The registered map (tests, diagnostics).
pub fn current() -> ImportMap {
    MAP.with(|m| m.borrow().clone())
}

/// "resolve a URL-like module specifier": `/`, `./`, `../` resolve against the base; otherwise an
/// absolute URL or nothing.
fn url_like(specifier: &str, base: &str) -> Option<String> {
    if specifier.starts_with('/') || specifier.starts_with("./") || specifier.starts_with("../") {
        let b = Url::parse(base).ok()?;
        return b.join(specifier).ok().map(|u| u.to_string());
    }
    Url::parse(specifier).ok().map(|u| u.to_string())
}

fn is_special_url(u: &str) -> bool {
    Url::parse(u).map(|x| url::is_special(x.scheme())).unwrap_or(false)
}

/// "sort and normalize a module specifier map".
fn normalize_map(obj: &serde_json::Map<String, serde_json::Value>, base: &str, warnings: &mut Vec<String>) -> SpecMap {
    let mut out: SpecMap = Vec::new();
    for (key, value) in obj {
        // "normalize a specifier key"
        if key.is_empty() {
            warnings.push(String::from("Invalid empty string specifier key."));
            continue;
        }
        let nkey = url_like(key, base).unwrap_or_else(|| key.clone());
        let addr = match value {
            serde_json::Value::String(s) => match url_like(s, base) {
                Some(u) => {
                    if key.ends_with('/') && !u.ends_with('/') {
                        warnings.push(format!("Invalid address \"{s}\" for package specifier key \"{key}\": must end with \"/\"."));
                        None
                    } else {
                        Some(u)
                    }
                }
                None => {
                    warnings.push(format!("Invalid address \"{s}\" for the specifier key \"{key}\"."));
                    None
                }
            },
            _ => {
                warnings.push(format!("Invalid address for the specifier key \"{key}\": addresses must be strings."));
                None
            }
        };
        out.retain(|(k, _)| *k != nkey);
        out.push((nkey, addr));
    }
    out.sort_by(|a, b| b.0.encode_utf16().cmp(a.0.encode_utf16()));
    out
}

/// "parse an import map string": a TypeError message for a structurally invalid map; warnings (the
/// console's) for the entries it drops.
pub fn parse(text: &str, base: &str) -> Result<(ImportMap, Vec<String>), String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("Failed to parse import map: {e}"))?;
    let obj = match &v {
        serde_json::Value::Object(o) => o,
        _ => return Err(String::from("An import map must be a JSON object.")),
    };
    let mut warnings = Vec::new();
    let mut map = ImportMap::default();
    if let Some(imp) = obj.get("imports") {
        match imp {
            serde_json::Value::Object(o) => map.imports = normalize_map(o, base, &mut warnings),
            _ => return Err(String::from("The \"imports\" top-level key of an import map must be a JSON object.")),
        }
    }
    if let Some(sc) = obj.get("scopes") {
        let o = match sc {
            serde_json::Value::Object(o) => o,
            _ => return Err(String::from("The \"scopes\" top-level key of an import map must be a JSON object.")),
        };
        for (prefix, m) in o {
            let mo = match m {
                serde_json::Value::Object(mo) => mo,
                _ => return Err(format!("The value of the scope \"{prefix}\" must be a JSON object.")),
            };
            let Some(scope) = Url::parse(base).ok().and_then(|b| b.join(prefix).ok()).map(|u| u.to_string()) else {
                warnings.push(format!("Invalid scope \"{prefix}\" (parsed against base URL \"{base}\")."));
                continue;
            };
            let nm = normalize_map(mo, base, &mut warnings);
            map.scopes.retain(|(s, _)| *s != scope);
            map.scopes.push((scope, nm));
        }
        map.scopes.sort_by(|a, b| b.0.encode_utf16().cmp(a.0.encode_utf16()));
    }
    if let Some(ig) = obj.get("integrity") {
        let o = match ig {
            serde_json::Value::Object(o) => o,
            _ => return Err(String::from("The \"integrity\" top-level key of an import map must be a JSON object.")),
        };
        for (k, v) in o {
            let Some(u) = url_like(k, base) else {
                warnings.push(format!("Invalid integrity key \"{k}\"."));
                continue;
            };
            match v {
                serde_json::Value::String(s) => map.integrity.push((u, s.clone())),
                _ => warnings.push(format!("Invalid integrity value for \"{k}\": must be a string.")),
            }
        }
    }
    for k in obj.keys() {
        if !matches!(k.as_str(), "imports" | "scopes" | "integrity") {
            warnings.push(format!("Invalid top-level key \"{k}\". Only \"imports\", \"scopes\" and \"integrity\" are allowed."));
        }
    }
    Ok((map, warnings))
}

/// Whether a rule `key` (in a scope `scope`, or top-level when None) would change how an already
/// resolved specifier resolved — such rules are dropped by the merge.
fn affects_resolved(scope: Option<&str>, key: &str) -> bool {
    RESOLVED.with(|r| {
        r.borrow().iter().any(|(base, spec, as_url)| {
            let in_scope = match scope {
                None => true,
                Some(s) => base == s || (s.ends_with('/') && base.starts_with(s)),
            };
            in_scope && (spec == key || (key.ends_with('/') && spec.starts_with(key) && as_url.as_deref().map(is_special_url).unwrap_or(true)))
        })
    })
}

/// "register an import map": parse, then "merge existing and new import maps" — a rule the existing map
/// already has, or one that would affect a module already resolved, is ignored (with a warning).
pub fn register(text: &str, base: &str) -> Result<Vec<String>, String> {
    let (new, mut warnings) = parse(text, base)?;
    MAP.with(|m| {
        let mut old = m.borrow_mut();
        for (scope, rules) in new.scopes {
            let pos = old.scopes.iter().position(|(s, _)| *s == scope);
            let mut merged: SpecMap = pos.map(|i| old.scopes[i].1.clone()).unwrap_or_default();
            for (k, v) in rules {
                if merged.iter().any(|(x, _)| *x == k) || affects_resolved(Some(&scope), &k) {
                    warnings.push(format!("An import map rule for specifier '{k}' was ignored as it conflicts with an existing rule."));
                    continue;
                }
                merged.push((k, v));
            }
            merged.sort_by(|a, b| b.0.encode_utf16().cmp(a.0.encode_utf16()));
            match pos {
                Some(i) => old.scopes[i].1 = merged,
                None => old.scopes.push((scope, merged)),
            }
        }
        old.scopes.sort_by(|a, b| b.0.encode_utf16().cmp(a.0.encode_utf16()));
        for (k, v) in new.imports {
            if old.imports.iter().any(|(x, _)| *x == k) || affects_resolved(None, &k) {
                warnings.push(format!("An import map rule for specifier '{k}' was ignored as it conflicts with an existing rule."));
                continue;
            }
            old.imports.push((k, v));
        }
        old.imports.sort_by(|a, b| b.0.encode_utf16().cmp(a.0.encode_utf16()));
        for (u, i) in new.integrity {
            if !old.integrity.iter().any(|(x, _)| *x == u) {
                old.integrity.push((u, i));
            }
        }
    });
    Ok(warnings)
}

/// "resolve an imports match".
fn imports_match(normalized: &str, as_url: Option<&str>, map: &SpecMap) -> Result<Option<String>, String> {
    for (key, addr) in map {
        if key == normalized {
            return match addr {
                Some(a) => Ok(Some(a.clone())),
                None => Err(format!("Failed to resolve module specifier \"{normalized}\": the import map blocks it.")),
            };
        }
        if key.ends_with('/') && normalized.starts_with(key.as_str()) && as_url.map(is_special_url).unwrap_or(true) {
            let Some(a) = addr else {
                return Err(format!("Failed to resolve module specifier \"{normalized}\": the import map blocks \"{key}\"."));
            };
            let after = &normalized[key.len()..];
            let u = Url::parse(a).ok().and_then(|b| b.join(after).ok()).map(|u| u.to_string());
            let Some(u) = u else {
                return Err(format!("Failed to resolve module specifier \"{normalized}\" through \"{key}\"."));
            };
            if !u.starts_with(a.as_str()) {
                return Err(format!("Failed to resolve module specifier \"{normalized}\": it backtracks above its prefix \"{key}\"."));
            }
            return Ok(Some(u));
        }
    }
    Ok(None)
}

/// HTML §8.1.5.3 "resolve a module specifier" against `base` (the referrer's URL, or the document's base
/// URL), through the page's import map; records the result in the resolved module set.
pub fn resolve(base: &str, specifier: &str) -> Result<String, String> {
    let as_url = url_like(specifier, base);
    let normalized = as_url.clone().unwrap_or_else(|| specifier.to_string());
    let serialized_base = Url::parse(base).map(|u| u.to_string()).unwrap_or_else(|_| base.to_string());
    let map = MAP.with(|m| m.borrow().clone());
    let mut result: Option<String> = None;
    for (prefix, scope_imports) in &map.scopes {
        if *prefix == serialized_base || (prefix.ends_with('/') && serialized_base.starts_with(prefix.as_str())) {
            if let Some(m) = imports_match(&normalized, as_url.as_deref(), scope_imports)? {
                result = Some(m);
                break;
            }
        }
    }
    if result.is_none() {
        result = imports_match(&normalized, as_url.as_deref(), &map.imports)?;
    }
    let result = match result.or(as_url.clone()) {
        Some(r) => r,
        None => {
            return Err(format!(
                "Failed to resolve module specifier \"{specifier}\". Relative references must start with either \"/\", \"./\", or \"../\"."
            ))
        }
    };
    RESOLVED.with(|r| {
        let mut r = r.borrow_mut();
        if r.len() < 4096 {
            r.push((serialized_base, normalized, as_url));
        }
    });
    Ok(result)
}

/// The integrity metadata the map gives `url` (HTML "resolve a module integrity metadata"), or "".
pub fn integrity_for(url: &str) -> String {
    MAP.with(|m| m.borrow().integrity.iter().find(|(u, _)| u == url).map(|(_, i)| i.clone()).unwrap_or_default())
}
