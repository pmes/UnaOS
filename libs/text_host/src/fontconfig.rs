//! The host's font configuration, read as data (AETHERFONT, SR61) — no libfontconfig.
//!
//! fontconfig's XML (fonts-conf(5)) is read for two things:
//! - **font directories**: `<dir>` (with `prefix="xdg"` → `$XDG_DATA_HOME`, default `~/.local/share`;
//!   `~` → `$HOME`; a relative path against the config file's directory), through `<include>`d files and
//!   directories (`conf.d/*.conf` in name order, `ignore_missing`);
//! - **family aliases**: `<alias><family>F</family><prefer>…</prefer><accept>…</accept><default>…</default>`
//!   rules in configuration order, which [`Config::expand`] applies to a family list exactly as fontconfig's
//!   pattern substitution does (prefer → before the matched family, accept → after it, default → at the end).
//!
//! - **hinting** (FONTHINT, SR62): `<match>` rules whose tests are only `family` (and `pixelsize`
//!   comparisons) and whose edits set `hinting` / `hintstyle` / `autohint` ([`Config::hint_mode`]), plus the
//!   `target="pattern"` `hintstyle` default (10-hinting-slight.conf) — what Chromium reads per face.
//!
//! Other `<match>` edits (lang tests, rgba), `<selectfont>`, `<cachedir>` and bindings are not evaluated.

use std::path::{Path, PathBuf};

/// A `<match target="font">` rule reduced to what decides hinting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HintRule {
    /// `<test name="family">` strings (any matches; empty = no family test).
    pub families: Vec<String>,
    /// `<test name="pixelsize" compare="less|less_eq|more|more_eq">`.
    pub pixelsize: Option<(String, f64)>,
    pub hinting: Option<bool>,
    /// 0 none, 1 slight, 2 medium, 3 full.
    pub hintstyle: Option<u8>,
    pub autohint: Option<bool>,
}

/// The hinting a face gets at a size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HintMode {
    pub hinting: bool,
    pub hintstyle: u8,
    pub autohint: bool,
}

/// One `<alias>` rule.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Alias {
    pub family: String,
    pub prefer: Vec<String>,
    pub accept: Vec<String>,
    pub default: Vec<String>,
}

/// The parsed configuration.
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub dirs: Vec<PathBuf>,
    pub aliases: Vec<Alias>,
    /// Files read, in order (for diagnostics).
    pub files: Vec<PathBuf>,
    /// Per-font hinting rules in configuration order.
    pub hint_rules: Vec<HintRule>,
    /// The pattern-level `hintstyle` default, when a config sets one.
    pub default_hintstyle: Option<u8>,
}

/// A minimal XML element tree: enough of XML 1.0 for fontconfig files (elements, attributes, character data,
/// comments, processing instructions, a DOCTYPE, the five predefined entities and numeric references).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    El(Element),
    Text(String),
}

impl Element {
    pub fn attr(&self, n: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str())
    }
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|c| if let Node::El(e) = c { Some(e) } else { None })
    }
    /// The concatenated character data of the element's subtree, trimmed.
    pub fn text(&self) -> String {
        fn walk(e: &Element, out: &mut String) {
            for c in &e.children {
                match c {
                    Node::Text(t) => out.push_str(t),
                    Node::El(x) => walk(x, out),
                }
            }
        }
        let mut s = String::new();
        walk(self, &mut s);
        s.trim().to_string()
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find(';') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let ent = &after[..j];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => out.push(c),
            None => {
                out.push('&');
                out.push_str(ent);
                out.push(';');
            }
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out
}

/// Parses an XML document into its root element. `None` when malformed.
pub fn parse_xml(src: &str) -> Option<Element> {
    let mut stack: Vec<Element> = vec![Element { name: String::new(), ..Default::default() }];
    let mut i = 0;
    let b = src.as_bytes();
    while i < b.len() {
        if b[i] != b'<' {
            let e = src[i..].find('<').map(|k| i + k).unwrap_or(b.len());
            let t = unescape(&src[i..e]);
            if !t.trim().is_empty() {
                stack.last_mut()?.children.push(Node::Text(t));
            }
            i = e;
            continue;
        }
        let rest = &src[i..];
        if rest.starts_with("<!--") {
            i += rest.find("-->")? + 3;
        } else if rest.starts_with("<?") {
            i += rest.find("?>")? + 2;
        } else if rest.starts_with("<![CDATA[") {
            let e = rest.find("]]>")?;
            stack.last_mut()?.children.push(Node::Text(rest[9..e].to_string()));
            i += e + 3;
        } else if rest.starts_with("<!") {
            // DOCTYPE (with an optional internal subset)
            let mut depth = 0i32;
            let mut k = 0;
            for (j, c) in rest.char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' => depth -= 1,
                    '>' if depth <= 0 => {
                        k = j;
                        break;
                    }
                    _ => {}
                }
            }
            i += k + 1;
        } else if rest.starts_with("</") {
            let e = rest.find('>')?;
            let name = rest[2..e].trim();
            let el = stack.pop()?;
            if el.name != name {
                return None;
            }
            stack.last_mut()?.children.push(Node::El(el));
            i += e + 1;
        } else {
            let e = tag_end(rest)?;
            let inner = &rest[1..e];
            let self_closing = inner.ends_with('/');
            let inner = inner.trim_end_matches('/');
            let (name, attrs) = parse_tag(inner)?;
            let el = Element { name, attrs, children: Vec::new() };
            if self_closing {
                stack.last_mut()?.children.push(Node::El(el));
            } else {
                stack.push(el);
            }
            i += e + 1;
        }
    }
    if stack.len() != 1 {
        return None;
    }
    let doc = stack.pop()?;
    doc.children.into_iter().find_map(|c| if let Node::El(e) = c { Some(e) } else { None })
}

/// Index of the `>` closing a start tag, skipping quoted attribute values.
fn tag_end(s: &str) -> Option<usize> {
    let mut q: Option<char> = None;
    for (j, c) in s.char_indices() {
        match (q, c) {
            (None, '"' | '\'') => q = Some(c),
            (Some(o), c) if c == o => q = None,
            (None, '>') => return Some(j),
            _ => {}
        }
    }
    None
}

fn parse_tag(s: &str) -> Option<(String, Vec<(String, String)>)> {
    let s = s.trim();
    let n_end = s.find(|c: char| c.is_whitespace()).unwrap_or(s.len());
    let name = s[..n_end].to_string();
    let mut attrs = Vec::new();
    let mut rest = s[n_end..].trim_start();
    while !rest.is_empty() {
        let eq = rest.find('=')?;
        let key = rest[..eq].trim().to_string();
        let v = rest[eq + 1..].trim_start();
        let q = v.chars().next()?;
        if q != '"' && q != '\'' {
            return None;
        }
        let close = v[1..].find(q)? + 1;
        attrs.push((key, unescape(&v[1..close])));
        rest = v[close + 1..].trim_start();
    }
    Some((name, attrs))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Resolves a `<dir>`/`<include>` path the way fontconfig does.
fn resolve(path: &str, prefix: Option<&str>, base: &Path) -> Option<PathBuf> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    match prefix {
        Some("xdg") => {
            let data = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| home().map(|h| h.join(".local/share")))?;
            return Some(data.join(path));
        }
        Some("relative") => return Some(base.join(path)),
        _ => {}
    }
    if let Some(r) = path.strip_prefix("~/") {
        return home().map(|h| h.join(r));
    }
    if path == "~" {
        return home();
    }
    let p = PathBuf::from(path);
    Some(if p.is_absolute() { p } else { base.join(p) })
}

impl Config {
    /// Reads the system configuration: `$FONTCONFIG_FILE`, else `$FONTCONFIG_PATH/fonts.conf`, else
    /// `/etc/fonts/fonts.conf`. An unreadable configuration gives the conventional directories.
    pub fn system() -> Config {
        let file = std::env::var_os("FONTCONFIG_FILE")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("FONTCONFIG_PATH").map(|p| PathBuf::from(p).join("fonts.conf")))
            .unwrap_or_else(|| PathBuf::from("/etc/fonts/fonts.conf"));
        let mut c = Config::default();
        c.read_file(&file, 0);
        if c.dirs.is_empty() {
            c.dirs.push(PathBuf::from("/usr/share/fonts"));
            c.dirs.push(PathBuf::from("/usr/local/share/fonts"));
            if let Some(h) = home() {
                c.dirs.push(h.join(".local/share/fonts"));
                c.dirs.push(h.join(".fonts"));
            }
        }
        c
    }

    /// Reads one configuration file (and what it includes).
    pub fn read_file(&mut self, path: &Path, depth: u32) {
        if depth > 8 || self.files.iter().any(|f| f == path) {
            return;
        }
        let Ok(src) = std::fs::read_to_string(path) else { return };
        self.files.push(path.to_path_buf());
        let Some(root) = parse_xml(&src) else { return };
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        self.read_element(&root, &base, depth);
    }

    /// Reads a configuration from XML text (tests); relative paths resolve against `base`.
    pub fn read_str(&mut self, src: &str, base: &Path) {
        if let Some(root) = parse_xml(src) {
            self.read_element(&root, base, 0);
        }
    }

    fn read_element(&mut self, root: &Element, base: &Path, depth: u32) {
        for e in root.elements() {
            match e.name.as_str() {
                "dir" => {
                    if let Some(p) = resolve(&e.text(), e.attr("prefix"), base) {
                        if !self.dirs.contains(&p) {
                            self.dirs.push(p);
                        }
                    }
                }
                "include" => {
                    let Some(p) = resolve(&e.text(), e.attr("prefix"), base) else { continue };
                    if p.is_dir() {
                        let mut files: Vec<PathBuf> = std::fs::read_dir(&p)
                            .map(|rd| {
                                rd.filter_map(|x| x.ok().map(|x| x.path()))
                                    .filter(|x| x.extension().is_some_and(|e| e == "conf"))
                                    .collect()
                            })
                            .unwrap_or_default();
                        files.sort();
                        for f in files {
                            self.read_file(&f, depth + 1);
                        }
                    } else {
                        self.read_file(&p, depth + 1);
                    }
                }
                "alias" => {
                    let fams = |name: &str| -> Vec<String> {
                        e.elements()
                            .filter(|x| x.name == name)
                            .flat_map(|x| x.elements().filter(|f| f.name == "family").map(|f| f.text()))
                            .filter(|s| !s.is_empty())
                            .collect()
                    };
                    let family = e.elements().find(|x| x.name == "family").map(|x| x.text()).unwrap_or_default();
                    if !family.is_empty() {
                        self.aliases.push(Alias {
                            family,
                            prefer: fams("prefer"),
                            accept: fams("accept"),
                            default: fams("default"),
                        });
                    }
                }
                "match" => self.read_hint_match(e),
                _ => {}
            }
        }
    }

    fn read_hint_match(&mut self, e: &Element) {
        let style_of = |x: &Element| -> Option<u8> {
            let c = x.elements().find(|c| c.name == "const")?.text();
            match c.as_str() {
                "hintnone" => Some(0),
                "hintslight" => Some(1),
                "hintmedium" => Some(2),
                "hintfull" => Some(3),
                _ => None,
            }
        };
        let bool_of = |x: &Element| -> Option<bool> {
            let b = x.elements().find(|c| c.name == "bool")?.text();
            match b.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            }
        };
        if e.attr("target") == Some("pattern") {
            // only the unconditional hintstyle default (10-hinting-slight.conf)
            if e.elements().any(|x| x.name == "test") {
                return;
            }
            if let Some(s) = e.elements().filter(|x| x.name == "edit" && x.attr("name") == Some("hintstyle")).find_map(style_of) {
                if self.default_hintstyle.is_none() {
                    self.default_hintstyle = Some(s);
                }
            }
            return;
        }
        if e.attr("target") != Some("font") {
            return;
        }
        let mut r = HintRule::default();
        for t in e.elements().filter(|x| x.name == "test") {
            match t.attr("name") {
                Some("family") => r.families.extend(t.elements().filter(|s| s.name == "string").map(|s| s.text())),
                Some("pixelsize") => {
                    let v = t.elements().find_map(|d| d.text().parse::<f64>().ok());
                    let (Some(c), Some(v)) = (t.attr("compare"), v) else { return };
                    r.pixelsize = Some((c.to_string(), v));
                }
                _ => return, // a test this reader does not evaluate: leave the rule out
            }
        }
        for ed in e.elements().filter(|x| x.name == "edit") {
            match ed.attr("name") {
                Some("hinting") => r.hinting = bool_of(ed),
                Some("hintstyle") => r.hintstyle = style_of(ed),
                Some("autohint") => r.autohint = bool_of(ed),
                _ => {}
            }
        }
        if r.hinting.is_some() || r.hintstyle.is_some() || r.autohint.is_some() {
            self.hint_rules.push(r);
        }
    }

    /// The hinting fontconfig gives a face of `family` at `pixelsize` px: the pattern default, then every rule
    /// whose tests pass, in configuration order.
    pub fn hint_mode(&self, family: &str, pixelsize: f64) -> HintMode {
        let mut m = HintMode { hinting: true, hintstyle: self.default_hintstyle.unwrap_or(1), autohint: false };
        for r in &self.hint_rules {
            if !r.families.is_empty() && !r.families.iter().any(|f| family_eq(f, family)) {
                continue;
            }
            if let Some((c, v)) = &r.pixelsize {
                let ok = match c.as_str() {
                    "less" => pixelsize < *v,
                    "less_eq" => pixelsize <= *v,
                    "more" => pixelsize > *v,
                    "more_eq" => pixelsize >= *v,
                    "eq" => pixelsize == *v,
                    _ => false,
                };
                if !ok {
                    continue;
                }
            }
            if let Some(h) = r.hinting {
                m.hinting = h;
            }
            if let Some(s) = r.hintstyle {
                m.hintstyle = s;
            }
            if let Some(a) = r.autohint {
                m.autohint = a;
            }
        }
        m
    }

    /// Applies every alias rule, in configuration order, to a family list (fontconfig compares family names
    /// case- and space-insensitively). A rule fires once, at the first family that matches it.
    pub fn expand(&self, families: &[&str]) -> Vec<String> {
        let mut list: Vec<String> = families.iter().map(|s| s.to_string()).collect();
        for a in &self.aliases {
            let Some(pos) = list.iter().position(|f| family_eq(f, &a.family)) else { continue };
            let mut out: Vec<String> = Vec::with_capacity(list.len() + a.prefer.len() + a.accept.len());
            out.extend(list[..pos].iter().cloned());
            out.extend(a.prefer.iter().cloned());
            out.push(list[pos].clone());
            out.extend(a.accept.iter().cloned());
            out.extend(list[pos + 1..].iter().cloned());
            out.extend(a.default.iter().cloned());
            list = out;
        }
        list
    }
}

/// fontconfig's family comparison: case-insensitive, ignoring spaces (FcStrCmpIgnoreBlanksAndCase).
pub fn family_eq(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect::<String>();
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_and_alias_kat() {
        let src = r#"<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <!-- comment <dir>nope</dir> -->
  <dir>/usr/share/fonts</dir>
  <dir prefix="xdg">fonts</dir>
  <dir>rel &amp; dir</dir>
  <alias binding="same"><family>sans-serif</family><prefer><family>A Sans</family><family>B</family></prefer></alias>
  <alias><family>B</family><accept><family>C</family></accept><default><family>D</family></default></alias>
  <alias><family>sans-serif</family><prefer><family>E</family></prefer></alias>
</fontconfig>"#;
        let mut c = Config::default();
        c.read_str(src, Path::new("/etc/fonts"));
        assert_eq!(c.dirs[0], PathBuf::from("/usr/share/fonts"));
        assert!(c.dirs[1].ends_with("fonts"));
        assert_eq!(c.dirs[2], PathBuf::from("/etc/fonts/rel & dir"));
        assert_eq!(c.aliases.len(), 3);
        // prefer goes before the matched family, accept after it, default at the very end;
        // a later rule sees the families earlier rules inserted.
        assert_eq!(c.expand(&["sans-serif"]), ["A Sans", "B", "C", "E", "sans-serif", "D"]);
        assert!(family_eq("DejaVu Sans", "dejavusans"));
        assert!(parse_xml("<a><b></a>").is_none());
    }

    #[test]
    fn hint_rule_kat() {
        let src = r#"<fontconfig>
  <match target="pattern"><edit name="hintstyle" mode="append"><const>hintslight</const></edit></match>
  <match target="font"><test qual="any" name="family"><string>WenQuanYi Zen Hei</string></test>
    <edit name="hinting" mode="assign"><bool>true</bool></edit>
    <edit name="hintstyle" mode="assign"><const>hintnone</const></edit></match>
  <match target="font"><test name="family"><string>Tiny</string></test>
    <test name="pixelsize" compare="less"><double>10</double></test>
    <edit name="hinting"><bool>false</bool></edit></match>
  <match target="font"><test name="lang"><string>th</string></test><edit name="hinting"><bool>false</bool></edit></match>
</fontconfig>"#;
        let mut c = Config::default();
        c.read_str(src, Path::new("/etc/fonts"));
        assert_eq!(c.default_hintstyle, Some(1));
        assert_eq!(c.hint_rules.len(), 2, "the lang-tested rule is not evaluated");
        assert_eq!(c.hint_mode("DejaVu Sans", 16.0), HintMode { hinting: true, hintstyle: 1, autohint: false });
        assert_eq!(c.hint_mode("WenQuanYi Zen Hei", 16.0).hintstyle, 0);
        assert!(!c.hint_mode("Tiny", 9.0).hinting);
        assert!(c.hint_mode("Tiny", 12.0).hinting);
    }
}
