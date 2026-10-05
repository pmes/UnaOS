//! An XML 1.0 tokenizer and tree builder sufficient for SVG: elements, attributes (with attribute-value
//! normalization, XML 1.0 §3.3.3), character and entity references (the five predefined entities, numeric
//! references, and general entities declared in the DOCTYPE internal subset — whose replacement text may itself
//! contain markup, which is re-parsed in place, §4.4), CDATA sections, comments, processing instructions, and
//! Namespaces in XML 1.0 (prefix scoping, default namespace). Not a validating parser: the DTD is read only for
//! its `<!ENTITY>` declarations. Malformed input yields `Error::Malformed`, never a panic.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const NS_SVG: &str = "http://www.w3.org/2000/svg";
pub const NS_XLINK: &str = "http://www.w3.org/1999/xlink";
pub const NS_XML: &str = "http://www.w3.org/XML/1998/namespace";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ns {
    None,
    Svg,
    Xlink,
    Xml,
    Other,
}

fn ns_of(uri: &str) -> Ns {
    match uri {
        NS_SVG => Ns::Svg,
        NS_XLINK => Ns::Xlink,
        NS_XML => Ns::Xml,
        "" => Ns::None,
        _ => Ns::Other,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attr {
    pub ns: Ns,
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Root,
    Element { ns: Ns, name: String },
    Text(String),
}

#[derive(Clone, Debug)]
pub struct Node {
    pub kind: Kind,
    pub attrs: Vec<Attr>,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
}

impl Node {
    pub fn name(&self) -> &str {
        match &self.kind {
            Kind::Element { name, .. } => name,
            _ => "",
        }
    }
    pub fn is_svg(&self, n: &str) -> bool {
        matches!(&self.kind, Kind::Element { ns: Ns::Svg, name } if name == n)
    }
    pub fn is_svg_element(&self) -> bool {
        matches!(&self.kind, Kind::Element { ns: Ns::Svg, .. })
    }
    /// An attribute in no namespace (the SVG attributes).
    pub fn attr(&self, n: &str) -> Option<&str> {
        self.attrs.iter().rev().find(|a| a.ns == Ns::None && a.name == n).map(|a| a.value.as_str())
    }
    pub fn set_attr(&mut self, n: &str, v: &str) {
        if let Some(a) = self.attrs.iter_mut().find(|a| a.ns == Ns::None && a.name == n) {
            a.value = v.to_string();
        } else {
            self.attrs.push(Attr { ns: Ns::None, name: n.to_string(), value: v.to_string() });
        }
    }
    /// `href` (SVG 2) or `xlink:href` (SVG 1.1); SVG 2 says plain `href` wins when both are present.
    pub fn href(&self) -> Option<&str> {
        self.attr("href")
            .or_else(|| self.attrs.iter().find(|a| a.ns == Ns::Xlink && a.name == "href").map(|a| a.value.as_str()))
    }
    pub fn text(&self) -> Option<&str> {
        match &self.kind {
            Kind::Text(t) => Some(t),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    pub nodes: Vec<Node>,
    pub ids: BTreeMap<String, usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Malformed(&'static str),
    NotUtf8,
    TooDeep,
    TooLarge,
}

const MAX_DEPTH: usize = 256;
const MAX_ENTITY_DEPTH: usize = 8;
const MAX_EXPANSION: usize = 4 << 20;
const MAX_NODES: usize = 1 << 20;

struct Parser {
    nodes: Vec<Node>,
    stack: Vec<usize>,
    /// Namespace bindings in scope: (prefix, uri, depth-of-stack when bound).
    bindings: Vec<(String, String, usize)>,
    entities: BTreeMap<String, String>,
    expanded: usize,
}

impl Document {
    pub fn parse(bytes: &[u8]) -> Result<Document, Error> {
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
        let s = core::str::from_utf8(bytes).map_err(|_| Error::NotUtf8)?;
        let mut p = Parser {
            nodes: alloc::vec![Node { kind: Kind::Root, attrs: Vec::new(), children: Vec::new(), parent: None }],
            stack: alloc::vec![0],
            bindings: alloc::vec![
                ("xml".to_string(), NS_XML.to_string(), 0),
                ("xlink".to_string(), NS_XLINK.to_string(), 0)
            ],
            entities: BTreeMap::new(),
            expanded: 0,
        };
        p.content(s, 0)?;
        let mut ids = BTreeMap::new();
        for (i, n) in p.nodes.iter().enumerate() {
            if let Some(id) = n.attr("id") {
                ids.entry(id.to_string()).or_insert(i);
            }
        }
        Ok(Document { nodes: p.nodes, ids })
    }

    pub fn root_element(&self) -> Option<usize> {
        self.nodes[0].children.iter().copied().find(|&c| matches!(self.nodes[c].kind, Kind::Element { .. }))
    }

    pub fn by_id(&self, id: &str) -> Option<usize> {
        self.ids.get(id).copied()
    }

    /// Element children of `n`.
    pub fn elements(&self, n: usize) -> impl Iterator<Item = usize> + '_ {
        self.nodes[n].children.iter().copied().filter(|&c| matches!(self.nodes[c].kind, Kind::Element { .. }))
    }

    pub fn is_ancestor(&self, anc: usize, mut n: usize) -> bool {
        while let Some(p) = self.nodes[n].parent {
            if p == anc {
                return true;
            }
            n = p;
        }
        false
    }
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':') || (c as u32) > 0x7f
}

impl Parser {
    fn cur(&self) -> usize {
        *self.stack.last().unwrap()
    }

    fn push_text(&mut self, t: &str) {
        if t.is_empty() {
            return;
        }
        let cur = self.cur();
        if let Some(&last) = self.nodes[cur].children.last() {
            if let Kind::Text(s) = &mut self.nodes[last].kind {
                s.push_str(t);
                return;
            }
        }
        let id = self.nodes.len();
        self.nodes.push(Node { kind: Kind::Text(t.to_string()), attrs: Vec::new(), children: Vec::new(), parent: Some(cur) });
        self.nodes[cur].children.push(id);
    }

    fn content(&mut self, s: &str, edepth: usize) -> Result<(), Error> {
        if edepth > MAX_ENTITY_DEPTH {
            return Err(Error::TooDeep);
        }
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            let next = b[i..].iter().position(|&c| c == b'<' || c == b'&').map(|p| i + p).unwrap_or(b.len());
            if next > i {
                if self.stack.len() > 1 {
                    self.push_text(&s[i..next]);
                }
                i = next;
                continue;
            }
            if b[i] == b'&' {
                let end = s[i..].find(';').ok_or(Error::Malformed("unterminated reference"))? + i;
                let name = &s[i + 1..end];
                i = end + 1;
                if let Some(c) = char_ref(name) {
                    let mut buf = [0u8; 4];
                    if self.stack.len() > 1 {
                        self.push_text(c.encode_utf8(&mut buf));
                    }
                } else if let Some(t) = predefined(name) {
                    if self.stack.len() > 1 {
                        self.push_text(t);
                    }
                } else if let Some(v) = self.entities.get(name).cloned() {
                    self.expanded += v.len();
                    if self.expanded > MAX_EXPANSION {
                        return Err(Error::TooLarge);
                    }
                    self.content(&v, edepth + 1)?;
                } else {
                    return Err(Error::Malformed("undeclared entity"));
                }
                continue;
            }
            // '<'
            let rest = &s[i..];
            if rest.starts_with("<!--") {
                let e = rest[4..].find("-->").ok_or(Error::Malformed("unterminated comment"))?;
                i += 4 + e + 3;
            } else if rest.starts_with("<![CDATA[") {
                let e = rest[9..].find("]]>").ok_or(Error::Malformed("unterminated CDATA"))?;
                if self.stack.len() > 1 {
                    self.push_text(&rest[9..9 + e]);
                }
                i += 9 + e + 3;
            } else if rest.starts_with("<!DOCTYPE") {
                i += self.doctype(rest)?;
            } else if rest.starts_with("<?") {
                let e = rest.find("?>").ok_or(Error::Malformed("unterminated PI"))?;
                i += e + 2;
            } else if rest.starts_with("</") {
                let e = rest.find('>').ok_or(Error::Malformed("unterminated end tag"))?;
                let name = rest[2..e].trim();
                if self.stack.len() <= 1 {
                    return Err(Error::Malformed("end tag without start"));
                }
                let top = self.cur();
                let open = self.qname_of(top);
                if open != name {
                    return Err(Error::Malformed("mismatched end tag"));
                }
                self.close_top();
                i += e + 1;
            } else {
                i += self.start_tag(rest, edepth)?;
            }
        }
        Ok(())
    }

    fn qname_of(&self, n: usize) -> String {
        // The raw qualified name is kept in a private attribute slot for end-tag matching.
        self.nodes[n].attrs.iter().find(|a| a.ns == Ns::Other && a.name == "\u{0}qname").map(|a| a.value.clone()).unwrap_or_default()
    }

    fn close_top(&mut self) {
        let depth = self.stack.len();
        self.bindings.retain(|b| b.2 < depth);
        let top = self.stack.pop().unwrap();
        self.nodes[top].attrs.retain(|a| !(a.ns == Ns::Other && a.name == "\u{0}qname"));
    }

    fn doctype(&mut self, s: &str) -> Result<usize, Error> {
        // <!DOCTYPE name ExternalID? [ internal subset ]? >
        let b = s.as_bytes();
        let mut i = 9;
        let mut quote = 0u8;
        while i < b.len() {
            let c = b[i];
            if quote != 0 {
                if c == quote {
                    quote = 0;
                }
            } else if c == b'"' || c == b'\'' {
                quote = c;
            } else if c == b'[' {
                let len = self.internal_subset(&s[i + 1..])?;
                i += 1 + len;
                continue;
            } else if c == b'>' {
                return Ok(i + 1);
            }
            i += 1;
        }
        Err(Error::Malformed("unterminated DOCTYPE"))
    }

    /// Reads declarations up to the closing ']'; returns bytes consumed including it.
    fn internal_subset(&mut self, s: &str) -> Result<usize, Error> {
        let mut i = 0;
        let b = s.as_bytes();
        while i < b.len() {
            let rest = &s[i..];
            if rest.starts_with(']') {
                return Ok(i + 1);
            } else if rest.starts_with("<!--") {
                let e = rest[4..].find("-->").ok_or(Error::Malformed("unterminated comment"))?;
                i += 4 + e + 3;
            } else if rest.starts_with("<!ENTITY") {
                let mut j = 8;
                let rb = rest.as_bytes();
                while j < rb.len() && rb[j].is_ascii_whitespace() {
                    j += 1;
                }
                let param = j < rb.len() && rb[j] == b'%';
                if param {
                    j += 1;
                    while j < rb.len() && rb[j].is_ascii_whitespace() {
                        j += 1;
                    }
                }
                let ns = j;
                while j < rb.len() && !rb[j].is_ascii_whitespace() {
                    j += 1;
                }
                let name = &rest[ns..j];
                while j < rb.len() && rb[j].is_ascii_whitespace() {
                    j += 1;
                }
                let mut value = None;
                if j < rb.len() && (rb[j] == b'"' || rb[j] == b'\'') {
                    let q = rb[j];
                    let e = rb[j + 1..].iter().position(|&c| c == q).ok_or(Error::Malformed("unterminated entity value"))?;
                    value = Some(&rest[j + 1..j + 1 + e]);
                    j += e + 2;
                }
                let e = rest[j..].find('>').ok_or(Error::Malformed("unterminated ENTITY"))?;
                if let (false, Some(v)) = (param, value) {
                    // Character references in an entity value are expanded at declaration (§4.5).
                    let v = expand_char_refs(v);
                    self.entities.entry(name.to_string()).or_insert(v);
                }
                i += j + e + 1;
            } else if rest.starts_with("<!") || rest.starts_with("<?") {
                // Other declarations: skip honouring quotes.
                let rb = rest.as_bytes();
                let mut j = 2;
                let mut q = 0u8;
                while j < rb.len() {
                    let c = rb[j];
                    if q != 0 {
                        if c == q {
                            q = 0;
                        }
                    } else if c == b'"' || c == b'\'' {
                        q = c;
                    } else if c == b'>' {
                        break;
                    }
                    j += 1;
                }
                i += j + 1;
            } else {
                i += rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            }
        }
        Err(Error::Malformed("unterminated internal subset"))
    }

    fn start_tag(&mut self, s: &str, _edepth: usize) -> Result<usize, Error> {
        let b = s.as_bytes();
        let mut i = 1;
        let ns_start = i;
        while i < b.len() && is_name_char(s[i..].chars().next().unwrap()) {
            i += s[i..].chars().next().unwrap().len_utf8();
        }
        let qname = &s[ns_start..i];
        if qname.is_empty() {
            return Err(Error::Malformed("bad start tag"));
        }
        let mut raw_attrs: Vec<(String, String)> = Vec::new();
        let self_closing;
        loop {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= b.len() {
                return Err(Error::Malformed("unterminated start tag"));
            }
            if b[i] == b'>' {
                self_closing = false;
                i += 1;
                break;
            }
            if b[i] == b'/' {
                if b.get(i + 1) != Some(&b'>') {
                    return Err(Error::Malformed("bad empty-element tag"));
                }
                self_closing = true;
                i += 2;
                break;
            }
            let an = i;
            while i < b.len() && is_name_char(s[i..].chars().next().unwrap()) {
                i += s[i..].chars().next().unwrap().len_utf8();
            }
            let aname = &s[an..i];
            if aname.is_empty() {
                return Err(Error::Malformed("bad attribute"));
            }
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if b.get(i) != Some(&b'=') {
                return Err(Error::Malformed("attribute without value"));
            }
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            let q = *b.get(i).ok_or(Error::Malformed("unterminated attribute"))?;
            if q != b'"' && q != b'\'' {
                return Err(Error::Malformed("unquoted attribute"));
            }
            let e = b[i + 1..].iter().position(|&c| c == q).ok_or(Error::Malformed("unterminated attribute"))?;
            let raw = &s[i + 1..i + 1 + e];
            i += e + 2;
            let v = self.attr_value(raw, 0)?;
            if raw_attrs.iter().any(|(n, _)| n == aname) {
                return Err(Error::Malformed("duplicate attribute"));
            }
            raw_attrs.push((aname.to_string(), v));
        }
        if self.stack.len() > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        if self.nodes.len() > MAX_NODES {
            return Err(Error::TooLarge);
        }
        if self.stack.len() == 1 && self.nodes[0].children.iter().any(|&c| matches!(self.nodes[c].kind, Kind::Element { .. })) {
            return Err(Error::Malformed("more than one root element"));
        }
        // Namespace declarations first (they scope this element too).
        let depth = self.stack.len() + 1;
        for (n, v) in &raw_attrs {
            if n == "xmlns" {
                self.bindings.push((String::new(), v.clone(), depth));
            } else if let Some(p) = n.strip_prefix("xmlns:") {
                self.bindings.push((p.to_string(), v.clone(), depth));
            }
        }
        let resolve = |bindings: &[(String, String, usize)], prefix: &str| -> Option<Ns> {
            bindings.iter().rev().find(|b| b.0 == prefix).map(|b| ns_of(&b.1))
        };
        let (ens, lname) = match qname.split_once(':') {
            Some((p, l)) => (resolve(&self.bindings, p).ok_or(Error::Malformed("unbound prefix"))?, l),
            None => (resolve(&self.bindings, "").unwrap_or(Ns::None), qname),
        };
        let mut attrs = Vec::with_capacity(raw_attrs.len() + 1);
        for (n, v) in raw_attrs {
            if n == "xmlns" || n.starts_with("xmlns:") {
                continue;
            }
            let (ans, an) = match n.split_once(':') {
                Some((p, l)) => (resolve(&self.bindings, p).unwrap_or(Ns::Other), l.to_string()),
                None => (Ns::None, n),
            };
            attrs.push(Attr { ns: ans, name: an, value: v });
        }
        attrs.push(Attr { ns: Ns::Other, name: "\u{0}qname".to_string(), value: qname.to_string() });
        let parent = self.cur();
        let id = self.nodes.len();
        self.nodes.push(Node { kind: Kind::Element { ns: ens, name: lname.to_string() }, attrs, children: Vec::new(), parent: Some(parent) });
        self.nodes[parent].children.push(id);
        self.stack.push(id);
        if self_closing {
            self.close_top();
        }
        Ok(i)
    }

    fn attr_value(&mut self, raw: &str, edepth: usize) -> Result<String, Error> {
        if edepth > MAX_ENTITY_DEPTH {
            return Err(Error::TooDeep);
        }
        let mut out = String::with_capacity(raw.len());
        let mut i = 0;
        let b = raw.as_bytes();
        while i < b.len() {
            let c = raw[i..].chars().next().unwrap();
            if c == '&' {
                let end = raw[i..].find(';').ok_or(Error::Malformed("unterminated reference"))? + i;
                let name = &raw[i + 1..end];
                if let Some(ch) = char_ref(name) {
                    out.push(ch);
                } else if let Some(t) = predefined(name) {
                    out.push_str(t);
                } else if let Some(v) = self.entities.get(name).cloned() {
                    self.expanded += v.len();
                    if self.expanded > MAX_EXPANSION {
                        return Err(Error::TooLarge);
                    }
                    let e = self.attr_value(&v, edepth + 1)?;
                    out.push_str(&e);
                } else {
                    return Err(Error::Malformed("undeclared entity"));
                }
                i = end + 1;
                continue;
            }
            if c == '<' {
                return Err(Error::Malformed("< in attribute value"));
            }
            out.push(if matches!(c, '\t' | '\n' | '\r') { ' ' } else { c });
            i += c.len_utf8();
        }
        Ok(out)
    }
}

fn predefined(name: &str) -> Option<&'static str> {
    Some(match name {
        "lt" => "<",
        "gt" => ">",
        "amp" => "&",
        "apos" => "'",
        "quot" => "\"",
        _ => return None,
    })
}

fn char_ref(name: &str) -> Option<char> {
    let n = name.strip_prefix('#')?;
    let v = if let Some(h) = n.strip_prefix('x').or_else(|| n.strip_prefix('X')) {
        u32::from_str_radix(h, 16).ok()?
    } else {
        n.parse::<u32>().ok()?
    };
    char::from_u32(v)
}

fn expand_char_refs(v: &str) -> String {
    let mut out = String::new();
    let mut rest = v;
    while let Some(p) = rest.find("&#") {
        out.push_str(&rest[..p]);
        let r = &rest[p..];
        if let Some(e) = r.find(';') {
            if let Some(c) = char_ref(&r[1..e]) {
                out.push(c);
                rest = &r[e + 1..];
                continue;
            }
        }
        out.push_str("&#");
        rest = &r[2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_attributes_namespaces() {
        let d = Document::parse(
            br##"<?xml version="1.0"?><!-- c --><svg xmlns="http://www.w3.org/2000/svg" xmlns:x="http://www.w3.org/1999/xlink" width="10">
            <g id="a"><use x:href="#a" y='2'/></g><foo:bar xmlns:foo="urn:z"/></svg>"##,
        )
        .unwrap();
        let r = d.root_element().unwrap();
        assert!(d.nodes[r].is_svg("svg"));
        assert_eq!(d.nodes[r].attr("width"), Some("10"));
        let g = d.by_id("a").unwrap();
        let u = d.elements(g).next().unwrap();
        assert!(d.nodes[u].is_svg("use"));
        assert_eq!(d.nodes[u].href(), Some("#a"));
        assert_eq!(d.nodes[u].attr("y"), Some("2"));
        let other = d.elements(r).nth(1).unwrap();
        assert_eq!(d.nodes[other].kind, Kind::Element { ns: Ns::Other, name: "bar".into() });
    }

    #[test]
    fn entities_cdata_and_normalization() {
        let d = Document::parse(
            br##"<!DOCTYPE svg [ <!ENTITY R "<rect id='r' width='&#53;'/>"> <!ENTITY c "red"> ]>
            <svg xmlns="http://www.w3.org/2000/svg" fill="&c;" a="x&#x9;y&lt;"><style><![CDATA[a<b]]></style>&R;</svg>"##,
        )
        .unwrap();
        let r = d.root_element().unwrap();
        assert_eq!(d.nodes[r].attr("fill"), Some("red"));
        assert_eq!(d.nodes[r].attr("a"), Some("x\ty<"));
        let st = d.elements(r).next().unwrap();
        let t = d.nodes[st].children[0];
        assert_eq!(d.nodes[t].text(), Some("a<b"));
        let rect = d.by_id("r").unwrap();
        assert_eq!(d.nodes[rect].attr("width"), Some("5"));
    }

    #[test]
    fn refusals() {
        assert!(Document::parse(b"<svg><g></svg>").is_err());
        assert!(Document::parse(b"<svg a='1' a='2'/>").is_err());
        assert!(Document::parse(b"<svg>&nope;</svg>").is_err());
        assert!(Document::parse(b"<a/><b/>").is_err());
        // Billion laughs is bounded.
        let mut s = String::from("<!DOCTYPE s [<!ENTITY a \"aaaaaaaaaa\">");
        let mut prev = 'a';
        for c in ['b', 'c', 'd', 'e', 'f', 'g', 'h'] {
            s.push_str(&alloc::format!("<!ENTITY {c} \"&{prev};&{prev};&{prev};&{prev};&{prev};&{prev};&{prev};&{prev};&{prev};&{prev};\">"));
            prev = c;
        }
        s.push_str("]><s>&h;</s>");
        assert!(Document::parse(s.as_bytes()).is_err());
    }
}
