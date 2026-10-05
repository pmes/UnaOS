//! A minimal CSS reader for SVG's `style` attribute and `<style>` element — **the seam for CSSCORE (SR47)**.
//! When css_core lands, [`parse_declarations`] and [`Stylesheet`] are the two calls it replaces; nothing else
//! in svg_core parses CSS.
//!
//! Covered: comments, declaration blocks (`;`-separated, quotes and parentheses respected, `!important`),
//! rule sets with selector lists, CSS Selectors Level 3 subset — type, universal, `#id`, `.class`, attribute
//! selectors (`[a]`, `=`, `~=`, `|=`, `^=`, `$=`, `*=`), `:first-child`, descendant and child combinators —
//! with specificity and source order. At-rules are skipped (their blocks too). Unknown pseudo-classes make a
//! selector match nothing, as a browser drops an invalid selector.

use crate::xml::{Document, Kind, Ns};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq)]
pub struct Declaration {
    pub name: String,
    pub value: String,
    pub important: bool,
}

pub fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find("/*") {
        out.push_str(&rest[..p]);
        match rest[p + 2..].find("*/") {
            Some(e) => rest = &rest[p + 2 + e + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Split at top-level occurrences of `sep` (outside quotes and parentheses).
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut q: Option<char> = None;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match q {
            Some(qc) => {
                if c == qc {
                    q = None;
                }
            }
            None => match c {
                '"' | '\'' => q = Some(c),
                '(' => depth += 1,
                ')' => depth -= 1,
                _ if c == sep && depth <= 0 => {
                    out.push(&s[start..i]);
                    start = i + c.len_utf8();
                }
                _ => {}
            },
        }
    }
    out.push(&s[start..]);
    out
}

/// `name: value; name: value !important; ...`
pub fn parse_declarations(s: &str) -> Vec<Declaration> {
    let s = strip_comments(s);
    let mut out = Vec::new();
    for d in split_top(&s, ';') {
        let Some((n, v)) = d.split_once(':') else { continue };
        let name = n.trim().to_ascii_lowercase();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            continue;
        }
        let mut value = v.trim();
        let mut important = false;
        if let Some(p) = value.to_ascii_lowercase().rfind("!important") {
            if value[p + 10..].trim().is_empty() {
                important = true;
                value = value[..p].trim_end();
            }
        }
        if value.is_empty() {
            continue;
        }
        out.push(Declaration { name, value: value.to_string(), important });
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
enum AttrOp {
    Exists,
    Eq(String),
    Word(String),
    Dash(String),
    Prefix(String),
    Suffix(String),
    Substr(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<(String, AttrOp)>,
    first_child: bool,
    never: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Comb {
    Descendant,
    Child,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Selector {
    /// Rightmost compound first; `combs[i]` joins `parts[i]` to `parts[i+1]` (its left neighbour).
    parts: Vec<Compound>,
    combs: Vec<Comb>,
    pub specificity: (u32, u32, u32),
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub selector: Selector,
    pub decls: Vec<Declaration>,
    pub order: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_' || (c as u32) > 0x7f
}

fn parse_compound(s: &str) -> Option<Compound> {
    let mut c = Compound::default();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let ident = |i: &mut usize| -> String {
        let st = *i;
        while *i < b.len() && is_ident(b[*i]) {
            *i += 1;
        }
        b[st..*i].iter().collect()
    };
    if i < b.len() && b[i] == '*' {
        i += 1;
    } else if i < b.len() && is_ident(b[i]) {
        let t = ident(&mut i);
        // Namespace prefix `svg|rect` → local name.
        c.tag = Some(t);
    }
    if i < b.len() && b[i] == '|' {
        i += 1;
        c.tag = if i < b.len() && b[i] == '*' {
            i += 1;
            None
        } else {
            Some(ident(&mut i))
        };
    }
    while i < b.len() {
        match b[i] {
            '#' => {
                i += 1;
                c.id = Some(ident(&mut i));
            }
            '.' => {
                i += 1;
                c.classes.push(ident(&mut i));
            }
            '[' => {
                let e = b[i..].iter().position(|&x| x == ']')? + i;
                let inner: String = b[i + 1..e].iter().collect();
                i = e + 1;
                let ops = ["~=", "|=", "^=", "$=", "*=", "="];
                let mut done = false;
                for op in ops {
                    if let Some((n, v)) = inner.split_once(op) {
                        let v = v.trim().trim_matches(|q| q == '"' || q == '\'').to_string();
                        let n = n.trim().to_string();
                        let o = match op {
                            "~=" => AttrOp::Word(v),
                            "|=" => AttrOp::Dash(v),
                            "^=" => AttrOp::Prefix(v),
                            "$=" => AttrOp::Suffix(v),
                            "*=" => AttrOp::Substr(v),
                            _ => AttrOp::Eq(v),
                        };
                        c.attrs.push((n, o));
                        done = true;
                        break;
                    }
                }
                if !done {
                    c.attrs.push((inner.trim().to_string(), AttrOp::Exists));
                }
            }
            ':' => {
                i += 1;
                if i < b.len() && b[i] == ':' {
                    i += 1;
                }
                let p = ident(&mut i).to_ascii_lowercase();
                if p == "first-child" {
                    c.first_child = true;
                } else {
                    c.never = true;
                }
                // Skip a functional pseudo's argument.
                if i < b.len() && b[i] == '(' {
                    let e = b[i..].iter().position(|&x| x == ')')? + i;
                    i = e + 1;
                    c.never = true;
                }
            }
            _ => return None,
        }
    }
    Some(c)
}

pub fn parse_selector(s: &str) -> Option<Selector> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Tokenize into compounds and combinators.
    let mut parts = Vec::new();
    let mut combs = Vec::new();
    let mut cur = String::new();
    let mut pending: Option<Comb> = None;
    let mut chars = s.chars().peekable();
    let mut in_bracket = false;
    while let Some(ch) = chars.next() {
        if in_bracket {
            cur.push(ch);
            if ch == ']' {
                in_bracket = false;
            }
            continue;
        }
        match ch {
            '[' => {
                in_bracket = true;
                cur.push(ch);
            }
            ' ' | '\t' | '\n' | '\r' | '>' => {
                if !cur.is_empty() {
                    parts.push(parse_compound(&cur)?);
                    cur.clear();
                    pending = Some(Comb::Descendant);
                }
                if ch == '>' {
                    if parts.is_empty() {
                        return None;
                    }
                    pending = Some(Comb::Child);
                }
            }
            '+' | '~' => return None, // sibling combinators: not supported → drop the rule
            _ => {
                if let Some(p) = pending.take() {
                    combs.push(p);
                }
                cur.push(ch);
            }
        }
    }
    if !cur.is_empty() {
        parts.push(parse_compound(&cur)?);
    }
    if parts.is_empty() || combs.len() + 1 != parts.len() {
        return None;
    }
    let mut spec = (0, 0, 0);
    for p in &parts {
        spec.0 += p.id.is_some() as u32;
        spec.1 += (p.classes.len() + p.attrs.len() + p.first_child as usize) as u32;
        spec.2 += p.tag.is_some() as u32;
    }
    parts.reverse();
    combs.reverse();
    Some(Selector { parts, combs, specificity: spec })
}

fn compound_matches(c: &Compound, doc: &Document, n: usize) -> bool {
    if c.never {
        return false;
    }
    let node = &doc.nodes[n];
    let Kind::Element { name, .. } = &node.kind else { return false };
    if let Some(t) = &c.tag {
        if t != name {
            return false;
        }
    }
    if let Some(id) = &c.id {
        if node.attr("id") != Some(id.as_str()) {
            return false;
        }
    }
    if !c.classes.is_empty() {
        let cls = node.attr("class").unwrap_or("");
        if !c.classes.iter().all(|k| cls.split_whitespace().any(|w| w == k)) {
            return false;
        }
    }
    for (an, op) in &c.attrs {
        let v = node.attrs.iter().find(|a| a.ns == Ns::None && &a.name == an).map(|a| a.value.as_str());
        let Some(v) = v else { return false };
        let ok = match op {
            AttrOp::Exists => true,
            AttrOp::Eq(x) => v == x,
            AttrOp::Word(x) => v.split_whitespace().any(|w| w == x),
            AttrOp::Dash(x) => v == x || v.starts_with(&alloc::format!("{x}-")),
            AttrOp::Prefix(x) => !x.is_empty() && v.starts_with(x.as_str()),
            AttrOp::Suffix(x) => !x.is_empty() && v.ends_with(x.as_str()),
            AttrOp::Substr(x) => !x.is_empty() && v.contains(x.as_str()),
        };
        if !ok {
            return false;
        }
    }
    if c.first_child {
        let Some(p) = node.parent else { return false };
        if doc.elements(p).next() != Some(n) {
            return false;
        }
    }
    true
}

impl Selector {
    pub fn matches(&self, doc: &Document, n: usize) -> bool {
        self.match_from(doc, n, 0)
    }
    fn match_from(&self, doc: &Document, n: usize, k: usize) -> bool {
        if !compound_matches(&self.parts[k], doc, n) {
            return false;
        }
        if k + 1 == self.parts.len() {
            return true;
        }
        match self.combs[k] {
            Comb::Child => match doc.nodes[n].parent {
                Some(p) => self.match_from(doc, p, k + 1),
                None => false,
            },
            Comb::Descendant => {
                let mut p = doc.nodes[n].parent;
                while let Some(a) = p {
                    if self.match_from(doc, a, k + 1) {
                        return true;
                    }
                    p = doc.nodes[a].parent;
                }
                false
            }
        }
    }
}

impl Stylesheet {
    pub fn parse_into(&mut self, css: &str) {
        let css = strip_comments(css);
        let b = css.as_bytes();
        let mut i = 0;
        while i < b.len() {
            while i < b.len() && (b[i] as char).is_whitespace() {
                i += 1;
            }
            if i >= b.len() {
                break;
            }
            if b[i] == b'@' {
                // At-rule: ends at ';' or after its balanced block.
                let mut j = i;
                let mut depth = 0;
                while j < b.len() {
                    match b[j] {
                        b';' if depth == 0 => {
                            j += 1;
                            break;
                        }
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth <= 0 {
                                j += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                i = j;
                continue;
            }
            let Some(open) = css[i..].find('{').map(|p| p + i) else { break };
            let Some(close) = css[open..].find('}').map(|p| p + open) else { break };
            let sel = &css[i..open];
            let decls = parse_declarations(&css[open + 1..close]);
            // An invalid selector in a list invalidates the whole rule (CSS Selectors §5).
            let parsed: Option<Vec<Selector>> = split_top(sel, ',').into_iter().map(parse_selector).collect();
            for selector in parsed.unwrap_or_default() {
                let order = self.rules.len();
                self.rules.push(Rule { selector, decls: decls.clone(), order });
            }
            i = close + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations() {
        let d = parse_declarations("fill: red; stroke:url(#a;b) ; /*x*/ opacity : 0.5 !important;;bad");
        assert_eq!(d.len(), 3);
        assert_eq!(d[1].value, "url(#a;b)");
        assert!(d[2].important);
        assert_eq!(d[2].value, "0.5");
    }

    #[test]
    fn selectors_match() {
        let doc = Document::parse(
            br##"<svg xmlns="http://www.w3.org/2000/svg"><g class="a b"><rect id="r" class="c" data-x="foo-bar"/><circle/></g></svg>"##,
        )
        .unwrap();
        let r = doc.by_id("r").unwrap();
        let circle = r + 1;
        let t = |s: &str, n| parse_selector(s).map(|x| x.matches(&doc, n)).unwrap_or(false);
        assert!(t("rect", r));
        assert!(t("*", r));
        assert!(t("#r", r));
        assert!(t(".a rect", r));
        assert!(t("g.a.b > .c", r));
        assert!(t("svg rect", r));
        assert!(!t("svg > rect", r));
        assert!(t("[data-x|=foo]", r));
        assert!(t("[data-x$=bar]", r));
        assert!(t("rect:first-child", r));
        assert!(!t("circle:first-child", circle));
        assert!(!t("rect:hover", r));
        assert_eq!(parse_selector("#a .b rect").unwrap().specificity, (1, 1, 1));
        let mut ss = Stylesheet::default();
        ss.parse_into("@import url(x.css); @media print { rect { fill: red } } rect, .c { fill: green }");
        assert_eq!(ss.rules.len(), 2);
    }
}
