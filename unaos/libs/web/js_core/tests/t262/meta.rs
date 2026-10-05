//! test262 frontmatter (INTERPRETING.md): flags, features, includes, negative expectations.

#[derive(Default, Debug, Clone)]
pub struct Meta {
    pub flags: Vec<String>,
    pub features: Vec<String>,
    pub includes: Vec<String>,
    pub negative_phase: Option<String>,
    pub negative_type: Option<String>,
}

impl Meta {
    pub fn has_flag(&self, f: &str) -> bool {
        self.flags.iter().any(|x| x == f)
    }
}

fn list(v: &str) -> Vec<String> {
    let v = v.trim();
    let v = v.strip_prefix('[').and_then(|x| x.strip_suffix(']')).unwrap_or(v);
    v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

pub fn parse_meta(src: &str) -> Meta {
    let mut m = Meta::default();
    let start = match src.find("/*---") {
        Some(s) => s + 5,
        None => return m,
    };
    let end = match src[start..].find("---*/") {
        Some(e) => start + e,
        None => return m,
    };
    let yaml = &src[start..end];
    let mut cur_key = String::new();
    for line in yaml.lines() {
        let indented = line.starts_with(' ') || line.starts_with('\t');
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if !indented {
            if let Some((k, v)) = t.split_once(':') {
                cur_key = k.trim().to_string();
                let v = v.trim();
                match cur_key.as_str() {
                    "flags" => m.flags = list(v),
                    "features" => m.features = list(v),
                    "includes" => m.includes = list(v),
                    _ => {}
                }
            }
            continue;
        }
        if let Some(item) = t.strip_prefix("- ") {
            match cur_key.as_str() {
                "flags" => m.flags.push(item.trim().to_string()),
                "features" => m.features.push(item.trim().to_string()),
                "includes" => m.includes.push(item.trim().to_string()),
                _ => {}
            }
            continue;
        }
        if cur_key == "negative" {
            if let Some((k, v)) = t.split_once(':') {
                match k.trim() {
                    "phase" => m.negative_phase = Some(v.trim().to_string()),
                    "type" => m.negative_type = Some(v.trim().to_string()),
                    _ => {}
                }
            }
        }
    }
    m
}

/// Features outside ECMAScript 2025 (proposals and post-2025 editions) plus host-specific hooks: tests needing
/// them are reported as skipped, separately from failures.
pub const OUT_OF_SCOPE: &[&str] = &[
    "Temporal", "ShadowRealm", "decorators", "explicit-resource-management", "import-defer", "source-phase-imports",
    "source-phase-imports-module-source", "iterator-sequencing", "Math.sumPrecise", "uint8array-base64", "Error.isError",
    "Array.fromAsync", "json-parse-with-source", "upsert", "joint-iteration", "immutable-arraybuffer", "canonical-tz",
    "legacy-regexp", "Atomics.waitAsync", "await-dictionary", "iterator-zip", "nonextensible-applies-to-private",
    "Atomics.pause", "IsHTMLDDA", "host-gc-required", "tail-call-optimization", "promise-allkeyed", "iterator-join",
    "import-text", "import-bytes", "Error.captureStackTrace", "regexp-v-flag-string-literal-escapes", "Intl.Locale-info",
];
