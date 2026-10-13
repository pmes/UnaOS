//! AETHERWPT4 (SR142): the XML parser behind `XMLHttpRequest`'s document response (XHR §3.6.10) and `DOMParser`
//! for the XML types (HTML §8.5.1 "parseFromString": "Create an XML parser … with XML scripting support disabled").
//!
//! A non-validating XML 1.0 (Fifth Edition) parser that enforces well-formedness and Namespaces in XML 1.0 (Third
//! Edition) namespace well-formedness: §2.2 Char, §2.3 Name / NameStartChar, §2.8 the XML declaration (version,
//! encoding, standalone) and the document type declaration with its internal subset (general entities with literal
//! values are kept — §4.2.1 — and expanded by §4.4.2 "Included"; parameter entities, element / attribute-list /
//! notation declarations are skipped as a non-validating processor may), §2.4 character data (`]]>` refused),
//! §2.5 comments (`--` refused), §2.6 processing instructions (target `xml` reserved), §2.7 CDATA sections, §3.1
//! tags (one root, matching end tags, unique attributes, `<` refused in an attribute value), §3.3.3 attribute-value
//! normalization, §4.1 character references (to a Char only) and the five predefined entities (§4.6), §2.11
//! end-of-line handling; Namespaces §3 (QName: at most one colon, non-empty parts), §4 (every prefix bound; `xml`
//! and `xmlns` reserved; an empty `xmlns:p` refused), §6.3 (no two attributes with the same expanded name).
//!
//! The result is a token stream the page's JS builds into a Document with the DOM's own `createElementNS` /
//! `setAttributeNS` (so namespaces live where the DOM keeps them); a well-formedness error is `Err` with the reason.

use std::collections::HashMap;

pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
pub const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

/// One parsed item, in document order. `Start` is followed by its content and a matching `End`.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Doctype { name: String, public_id: String, system_id: String },
    /// (namespace, qualified name, attributes as (namespace, qualified name, value)).
    Start { ns: Option<String>, qname: String, attrs: Vec<(Option<String>, String, String)> },
    End,
    Text(String),
    CData(String),
    Comment(String),
    Pi { target: String, data: String },
}

/// What a document parse hands back: the items and the XML declaration's `encoding`, if any.
#[derive(Debug, Default)]
pub struct Parsed {
    pub toks: Vec<Tok>,
    pub encoding: Option<String>,
}

fn is_char(c: char) -> bool {
    matches!(c, '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

fn is_name_start(c: char) -> bool {
    matches!(c,
        ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// Namespaces §3 QName: `Prefix ':' LocalPart` or `LocalPart`, each an NCName.
fn split_qname(q: &str) -> Result<(Option<&str>, &str), String> {
    match q.split_once(':') {
        None => Ok((None, q)),
        Some((p, l)) => {
            if p.is_empty() || l.is_empty() || l.contains(':') || !l.starts_with(is_name_start) {
                return Err(format!("not a QName: {q}"));
            }
            Ok((Some(p), l))
        }
    }
}

/// An entity expansion bound (§4.3.2 forbids recursion; this bounds the size of an exponential one too).
const EXPANSION_LIMIT: usize = 1 << 20;

struct P {
    c: Vec<char>,
    i: usize,
    entities: HashMap<String, String>,
    expanded: usize,
    out: Vec<Tok>,
    /// The open elements' qualified names and the namespace bindings each one added.
    stack: Vec<(String, Vec<(Option<String>, Option<String>)>)>,
    /// In-scope bindings, innermost last: (prefix — None for the default namespace, namespace — None for "no namespace").
    scope: Vec<(Option<String>, Option<String>)>,
}

type R<T> = Result<T, String>;

impl P {
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }
    fn at(&self, s: &str) -> bool {
        let mut k = self.i;
        for ch in s.chars() {
            if self.c.get(k) != Some(&ch) {
                return false;
            }
            k += 1;
        }
        true
    }
    fn eat(&mut self, s: &str) -> bool {
        if self.at(s) {
            self.i += s.chars().count();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, s: &str) -> R<()> {
        if self.eat(s) {
            Ok(())
        } else {
            Err(format!("expected {s:?} at {}", self.i))
        }
    }
    fn skip_space(&mut self) -> bool {
        let s = self.i;
        while self.peek().is_some_and(is_space) {
            self.i += 1;
        }
        self.i > s
    }
    fn name(&mut self) -> R<String> {
        match self.peek() {
            Some(c) if is_name_start(c) => {}
            _ => return Err(format!("expected a name at {}", self.i)),
        }
        let s = self.i;
        while self.peek().is_some_and(is_name_char) {
            self.i += 1;
        }
        Ok(self.c[s..self.i].iter().collect())
    }
    /// The text up to `end` (consumed), each character a Char.
    fn until(&mut self, end: &str) -> R<String> {
        let s = self.i;
        while !self.at(end) {
            match self.peek() {
                None => return Err(format!("unterminated construct, expected {end:?}")),
                Some(c) if !is_char(c) => return Err(format!("not a Char: U+{:04X}", c as u32)),
                _ => self.i += 1,
            }
        }
        let t: String = self.c[s..self.i].iter().collect();
        self.i += end.chars().count();
        Ok(t)
    }
    fn quoted(&mut self) -> R<String> {
        let q = match self.peek() {
            Some(q @ ('"' | '\'')) => q,
            _ => return Err(format!("expected a quoted literal at {}", self.i)),
        };
        self.i += 1;
        let s = self.i;
        while self.peek() != Some(q) {
            match self.peek() {
                None => return Err("unterminated literal".into()),
                Some(c) if !is_char(c) => return Err("not a Char in a literal".into()),
                _ => self.i += 1,
            }
        }
        let t: String = self.c[s..self.i].iter().collect();
        self.i += 1;
        Ok(t)
    }

    /// §4.1 CharRef after the `&#` (consumed): the character.
    fn char_ref(&mut self) -> R<char> {
        let hex = self.eat("x");
        let s = self.i;
        while self.peek().is_some_and(|c| if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() }) {
            self.i += 1;
        }
        if self.i == s {
            return Err("empty character reference".into());
        }
        let digits: String = self.c[s..self.i].iter().collect();
        self.expect(";")?;
        let v = u32::from_str_radix(&digits, if hex { 16 } else { 10 }).map_err(|_| "character reference out of range".to_string())?;
        char::from_u32(v).filter(|c| is_char(*c)).ok_or_else(|| format!("character reference to a non-Char #{v}"))
    }

    /// §4.6 / §4.2.1: the replacement text of the general entity `name`.
    fn entity(&mut self, name: &str) -> R<String> {
        let t = match name {
            "lt" => "<".to_string(),
            "gt" => ">".to_string(),
            "amp" => "&".to_string(),
            "apos" => "'".to_string(),
            "quot" => "\"".to_string(),
            _ => self.entities.get(name).cloned().ok_or_else(|| format!("undefined entity &{name};"))?,
        };
        self.expanded += t.len();
        if self.expanded > EXPANSION_LIMIT {
            return Err("entity expansion limit".into());
        }
        Ok(t)
    }

    /// §3.3.3 attribute-value normalization of a quoted literal at the cursor (CDATA type: no further collapsing).
    fn att_value(&mut self) -> R<String> {
        let q = match self.peek() {
            Some(q @ ('"' | '\'')) => q,
            _ => return Err(format!("expected an attribute value at {}", self.i)),
        };
        self.i += 1;
        let mut out = String::new();
        let mut depth_guard = 0usize;
        loop {
            match self.peek() {
                None => return Err("unterminated attribute value".into()),
                Some(c) if c == q => {
                    self.i += 1;
                    break;
                }
                Some('<') => return Err("'<' in an attribute value".into()),
                Some('&') => {
                    self.i += 1;
                    if self.eat("#") {
                        out.push(self.char_ref()?);
                    } else {
                        let n = self.name()?;
                        self.expect(";")?;
                        let rep = self.entity(&n)?;
                        depth_guard += 1;
                        if depth_guard > 100_000 {
                            return Err("entity expansion limit".into());
                        }
                        out.push_str(&self.normalize_replacement(&rep, 0)?);
                    }
                }
                Some(c) if is_space(c) => {
                    out.push(' ');
                    self.i += 1;
                }
                Some(c) if !is_char(c) => return Err("not a Char in an attribute value".into()),
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
        Ok(out)
    }

    /// An entity's replacement text inside an attribute value (§3.3.3: references in it are expanded recursively).
    fn normalize_replacement(&mut self, rep: &str, depth: usize) -> R<String> {
        if depth > 32 {
            return Err("recursive entity".into());
        }
        let ch: Vec<char> = rep.chars().collect();
        let mut out = String::new();
        let mut k = 0;
        while k < ch.len() {
            match ch[k] {
                '<' => return Err("'<' in an attribute value (through an entity)".into()),
                '&' => {
                    let rest: String = ch[k + 1..].iter().collect();
                    let end = rest.find(';').ok_or("unterminated reference")?;
                    let body = &rest[..end];
                    if let Some(num) = body.strip_prefix('#') {
                        let (hex, d) = match num.strip_prefix('x') {
                            Some(h) => (true, h),
                            None => (false, num),
                        };
                        let v = u32::from_str_radix(d, if hex { 16 } else { 10 }).map_err(|_| "bad character reference".to_string())?;
                        out.push(char::from_u32(v).filter(|c| is_char(*c)).ok_or("character reference to a non-Char")?);
                    } else {
                        let r = self.entity(body)?;
                        out.push_str(&self.normalize_replacement(&r, depth + 1)?);
                    }
                    k += 1 + body.chars().count() + 1;
                }
                c if is_space(c) => {
                    out.push(' ');
                    k += 1;
                }
                c => {
                    out.push(c);
                    k += 1;
                }
            }
        }
        Ok(out)
    }

    fn push_text(&mut self, t: &str) {
        if t.is_empty() {
            return;
        }
        if let Some(Tok::Text(prev)) = self.out.last_mut() {
            prev.push_str(t);
        } else {
            self.out.push(Tok::Text(t.to_string()));
        }
    }

    /// §2.5 after `<!--`.
    fn comment(&mut self) -> R<()> {
        let t = self.until("--")?;
        if !self.eat(">") {
            return Err("'--' inside a comment".into());
        }
        self.out.push(Tok::Comment(t));
        Ok(())
    }

    /// §2.6 after `<?`.
    fn pi(&mut self) -> R<()> {
        let target = self.name()?;
        if target.eq_ignore_ascii_case("xml") {
            return Err("processing instruction target 'xml' is reserved".into());
        }
        if target.contains(':') {
            return Err("a processing instruction target contains a colon".into());
        }
        let data = if self.eat("?>") {
            String::new()
        } else {
            if !self.skip_space() {
                return Err("expected whitespace after the PI target".into());
            }
            self.until("?>")?
        };
        self.out.push(Tok::Pi { target, data });
        Ok(())
    }

    /// Misc (§2.8): comments, PIs and white space.
    fn misc(&mut self) -> R<()> {
        loop {
            self.skip_space();
            if self.eat("<!--") {
                self.comment()?;
            } else if self.at("<?") {
                self.i += 2;
                self.pi()?;
            } else {
                return Ok(());
            }
        }
    }

    /// §2.8 XMLDecl after `<?xml` (the caller checked a space follows): version, encoding, standalone.
    fn xml_decl(&mut self) -> R<Option<String>> {
        self.skip_space();
        let mut enc = None;
        let mut first = true;
        let mut seen: Vec<String> = Vec::new();
        loop {
            self.skip_space();
            if self.eat("?>") {
                break;
            }
            let n = self.name()?;
            self.skip_space();
            self.expect("=")?;
            self.skip_space();
            let v = self.quoted()?;
            match n.as_str() {
                "version" if first => {
                    if !(v.starts_with("1.") && v.len() > 2 && v[2..].chars().all(|c| c.is_ascii_digit())) {
                        return Err(format!("unsupported XML version {v}"));
                    }
                }
                "encoding" if seen.iter().any(|s| s == "version") && !seen.iter().any(|s| s == "standalone") => {
                    let ok = v.starts_with(|c: char| c.is_ascii_alphabetic()) && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
                    if !ok {
                        return Err(format!("bad encoding name {v}"));
                    }
                    enc = Some(v);
                }
                "standalone" if seen.iter().any(|s| s == "version") => {
                    if v != "yes" && v != "no" {
                        return Err("standalone must be yes or no".into());
                    }
                }
                _ => return Err(format!("unexpected {n} in the XML declaration")),
            }
            if seen.contains(&n) {
                return Err(format!("{n} twice in the XML declaration"));
            }
            seen.push(n);
            first = false;
            if !self.at("?>") && !self.peek().is_some_and(is_space) {
                return Err("expected whitespace in the XML declaration".into());
            }
        }
        if !seen.iter().any(|s| s == "version") {
            return Err("the XML declaration has no version".into());
        }
        Ok(enc)
    }

    /// §2.8 doctypedecl after `<!DOCTYPE`.
    fn doctype(&mut self) -> R<()> {
        if !self.skip_space() {
            return Err("expected whitespace after <!DOCTYPE".into());
        }
        let name = self.name()?;
        let had_space = self.skip_space();
        let (mut public_id, mut system_id) = (String::new(), String::new());
        if had_space && self.eat("SYSTEM") {
            self.skip_space();
            system_id = self.quoted()?;
        } else if had_space && self.eat("PUBLIC") {
            self.skip_space();
            public_id = self.quoted()?;
            if !public_id.chars().all(|c| matches!(c, ' ' | '\r' | '\n' | 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '\'' | '(' | ')' | '+' | ',' | '.' | '/' | ':' | '=' | '?' | ';' | '!' | '*' | '#' | '@' | '$' | '_' | '%')) {
                return Err("bad public identifier".into());
            }
            self.skip_space();
            system_id = self.quoted()?;
        }
        self.skip_space();
        if self.eat("[") {
            self.internal_subset()?;
            self.skip_space();
        }
        self.expect(">")?;
        self.out.push(Tok::Doctype { name, public_id, system_id });
        Ok(())
    }

    /// The internal subset up to and including `]`: general entity declarations kept, everything else skipped.
    fn internal_subset(&mut self) -> R<()> {
        loop {
            self.skip_space();
            if self.eat("]") {
                return Ok(());
            }
            if self.eat("<!--") {
                let mark = self.out.len();
                self.comment()?;
                self.out.truncate(mark);
            } else if self.at("<?") {
                self.i += 2;
                let mark = self.out.len();
                self.pi()?;
                self.out.truncate(mark);
            } else if self.eat("<!ENTITY") {
                if !self.skip_space() {
                    return Err("expected whitespace in an entity declaration".into());
                }
                let parameter = self.eat("%");
                if parameter {
                    self.skip_space();
                }
                let n = self.name()?;
                self.skip_space();
                if matches!(self.peek(), Some('"' | '\'')) {
                    let v = self.quoted()?;
                    if !parameter && !self.entities.contains_key(&n) {
                        // §4.5: character references in an EntityValue are expanded at declaration.
                        let v = expand_char_refs(&v)?;
                        self.entities.insert(n, v);
                    }
                } else {
                    // An external entity: not read (a non-validating processor need not), so not declared here.
                    self.skip_decl()?;
                    continue;
                }
                self.skip_space();
                self.expect(">")?;
            } else if self.eat("<!") {
                self.skip_decl()?;
            } else if self.eat("%") {
                self.name()?;
                self.expect(";")?;
            } else {
                return Err(format!("unexpected content in the internal subset at {}", self.i));
            }
        }
    }

    /// A markup declaration's rest, to its `>` (quoted literals may contain `>`).
    fn skip_decl(&mut self) -> R<()> {
        loop {
            match self.peek() {
                None => return Err("unterminated declaration".into()),
                Some('>') => {
                    self.i += 1;
                    return Ok(());
                }
                Some('"' | '\'') => {
                    self.quoted()?;
                }
                _ => self.i += 1,
            }
        }
    }

    fn lookup(&self, prefix: Option<&str>) -> Option<Option<String>> {
        if prefix == Some("xml") {
            return Some(Some(XML_NS.to_string()));
        }
        self.scope.iter().rev().find(|(p, _)| p.as_deref() == prefix).map(|(_, n)| n.clone())
    }

    /// §3.1 STag / EmptyElemTag after `<`, with the Namespaces checks.
    fn start_tag(&mut self) -> R<()> {
        let qname = self.name()?;
        let mut raw: Vec<(String, String)> = Vec::new();
        let empty;
        loop {
            let sp = self.skip_space();
            if self.eat("/>") {
                empty = true;
                break;
            }
            if self.eat(">") {
                empty = false;
                break;
            }
            if !sp {
                return Err(format!("expected whitespace before an attribute at {}", self.i));
            }
            let n = self.name()?;
            self.skip_space();
            self.expect("=")?;
            self.skip_space();
            let v = self.att_value()?;
            if raw.iter().any(|(x, _)| *x == n) {
                return Err(format!("attribute {n} twice"));
            }
            raw.push((n, v));
        }
        // Namespaces §4: the declarations first.
        let mut added: Vec<(Option<String>, Option<String>)> = Vec::new();
        for (n, v) in &raw {
            if n == "xmlns" {
                if v == XML_NS || v == XMLNS_NS {
                    return Err("the default namespace may not be the xml or xmlns namespace".into());
                }
                added.push((None, if v.is_empty() { None } else { Some(v.clone()) }));
            } else if let Some(p) = n.strip_prefix("xmlns:") {
                split_qname(n)?;
                if p == "xmlns" {
                    return Err("the xmlns prefix may not be declared".into());
                }
                if (p == "xml") != (v == XML_NS) {
                    return Err("the xml prefix is bound to the XML namespace only".into());
                }
                if v == XMLNS_NS {
                    return Err("the xmlns namespace may not be bound".into());
                }
                if v.is_empty() {
                    return Err(format!("empty namespace for prefix {p}"));
                }
                added.push((Some(p.to_string()), Some(v.clone())));
            }
        }
        self.scope.extend(added.iter().cloned());
        let (ep, _) = split_qname(&qname)?;
        let ns = match ep {
            Some("xmlns") => return Err("an element may not have the xmlns prefix".into()),
            Some(p) => self.lookup(Some(p)).ok_or_else(|| format!("unbound prefix {p}"))?,
            None => self.lookup(None).flatten(),
        };
        let mut attrs: Vec<(Option<String>, String, String)> = Vec::new();
        let mut expanded: Vec<(Option<String>, String)> = Vec::new();
        for (n, v) in raw {
            let (p, local) = split_qname(&n)?;
            let ans = if n == "xmlns" || p == Some("xmlns") {
                Some(XMLNS_NS.to_string())
            } else {
                match p {
                    None => None,
                    Some(p) => self.lookup(Some(p)).ok_or_else(|| format!("unbound prefix {p}"))?.or_else(|| Some(String::new())),
                }
            };
            let key = (ans.clone(), if n == "xmlns" { "xmlns".to_string() } else { local.to_string() });
            if expanded.contains(&key) {
                return Err(format!("two attributes named {{{}}}{}", key.0.clone().unwrap_or_default(), key.1));
            }
            expanded.push(key);
            attrs.push((ans, n, v));
        }
        self.out.push(Tok::Start { ns, qname: qname.clone(), attrs });
        if empty {
            for _ in 0..added.len() {
                self.scope.pop();
            }
            self.out.push(Tok::End);
        } else {
            self.stack.push((qname, added));
        }
        Ok(())
    }

    fn end_tag(&mut self) -> R<()> {
        let n = self.name()?;
        self.skip_space();
        self.expect(">")?;
        let Some((open, added)) = self.stack.pop() else { return Err("end tag without a start tag".into()) };
        if open != n {
            return Err(format!("end tag {n} does not match {open}"));
        }
        for _ in 0..added.len() {
            self.scope.pop();
        }
        self.out.push(Tok::End);
        Ok(())
    }

    /// The root element and everything in it.
    fn element(&mut self) -> R<()> {
        if !self.eat("<") {
            return Err("expected the root element".into());
        }
        self.start_tag()?;
        let mut text = String::new();
        while !self.stack.is_empty() {
            match self.peek() {
                None => return Err("unclosed element".into()),
                Some('<') => {
                    let t = std::mem::take(&mut text);
                    self.push_text(&t);
                    if self.eat("</") {
                        self.end_tag()?;
                    } else if self.eat("<!--") {
                        self.comment()?;
                    } else if self.eat("<![CDATA[") {
                        let t = self.until("]]>")?;
                        self.out.push(Tok::CData(t));
                    } else if self.eat("<?") {
                        self.pi()?;
                    } else {
                        self.i += 1;
                        self.start_tag()?;
                    }
                }
                Some('&') => {
                    self.i += 1;
                    if self.eat("#") {
                        text.push(self.char_ref()?);
                    } else {
                        let n = self.name()?;
                        self.expect(";")?;
                        let rep = self.entity(&n)?;
                        if matches!(n.as_str(), "lt" | "gt" | "amp" | "apos" | "quot") {
                            text.push_str(&rep);
                        } else {
                            // §4.4.2 Included: the replacement text is parsed in place of the reference.
                            let rc: Vec<char> = rep.chars().collect();
                            self.c.splice(self.i..self.i, rc);
                        }
                    }
                }
                Some(c) => {
                    if !is_char(c) {
                        return Err(format!("not a Char: U+{:04X}", c as u32));
                    }
                    if self.at("]]>") {
                        return Err("']]>' in character data".into());
                    }
                    text.push(c);
                    self.i += 1;
                }
            }
        }
        self.push_text(&text);
        Ok(())
    }
}

fn expand_char_refs(v: &str) -> R<String> {
    let mut out = String::new();
    let mut rest = v;
    while let Some(k) = rest.find("&#") {
        out.push_str(&rest[..k]);
        let after = &rest[k + 2..];
        let end = after.find(';').ok_or("unterminated character reference")?;
        let body = &after[..end];
        let (hex, d) = match body.strip_prefix('x') {
            Some(h) => (true, h),
            None => (false, body),
        };
        let n = u32::from_str_radix(d, if hex { 16 } else { 10 }).map_err(|_| "bad character reference".to_string())?;
        out.push(char::from_u32(n).filter(|c| is_char(*c)).ok_or("character reference to a non-Char")?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// §2.11: CR LF and a lone CR become LF.
fn normalize_newlines(s: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\r' {
            if it.peek() == Some(&'\n') {
                it.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// Parses a whole XML document (already decoded; a leading U+FEFF is the BOM and dropped).
pub fn parse(src: &str) -> Result<Parsed, String> {
    let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let mut p = P { c: normalize_newlines(src), i: 0, entities: HashMap::new(), expanded: 0, out: Vec::new(), stack: Vec::new(), scope: Vec::new() };
    let mut encoding = None;
    if p.at("<?xml") && p.c.get(5).is_some_and(|c| is_space(*c)) {
        p.i += 5;
        encoding = p.xml_decl()?;
    }
    p.misc()?;
    if p.eat("<!DOCTYPE") {
        p.doctype()?;
        p.misc()?;
    }
    p.element()?;
    p.misc()?;
    if p.i < p.c.len() {
        return Err(format!("content after the root element at {}", p.i));
    }
    Ok(Parsed { toks: p.out, encoding })
}

/// The `encoding` of a leading XML declaration in `bytes` (an ASCII-compatible prefix), for XHR §3.6.9's "rules set forth
/// in the XML specifications" (XML §4.3.3 / Appendix F: the declaration names the encoding when no BOM or transport
/// charset does).
pub fn sniff_declared_encoding(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(1024)];
    if !head.starts_with(b"<?xml") {
        return None;
    }
    let end = head.windows(2).position(|w| w == b"?>")?;
    let decl = std::str::from_utf8(&head[5..end]).ok()?;
    let k = decl.find("encoding")?;
    let rest = decl[k + 8..].trim_start().strip_prefix('=')?.trim_start();
    let q = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let v = &rest[1..];
    let e = v.find(q)?;
    Some(v[..e].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_and_namespaces() {
        let p = parse("<?xml version='1.0' encoding='windows-1252'?><!DOCTYPE r [<!ENTITY e 'v&#65;'>]><r xmlns='urn:a' xmlns:p='urn:p' p:x='1 \t2'><p:c>&e;&lt;<![CDATA[<k>]]></p:c><?pi d?><!--c--></r>").unwrap();
        assert_eq!(p.encoding.as_deref(), Some("windows-1252"));
        assert!(matches!(&p.toks[0], Tok::Doctype { name, .. } if name == "r"));
        match &p.toks[1] {
            Tok::Start { ns, qname, attrs } => {
                assert_eq!(ns.as_deref(), Some("urn:a"));
                assert_eq!(qname, "r");
                assert_eq!(attrs[2], (Some("urn:p".into()), "p:x".into(), "1  2".into()));
                assert_eq!(attrs[0].0.as_deref(), Some(XMLNS_NS));
            }
            t => panic!("{t:?}"),
        }
        assert!(matches!(&p.toks[2], Tok::Start { ns, .. } if ns.as_deref() == Some("urn:p")));
        assert_eq!(p.toks[3], Tok::Text("vA<".into()));
        assert_eq!(p.toks[4], Tok::CData("<k>".into()));
    }

    #[test]
    fn not_well_formed() {
        for bad in ["<x", "<x></x", "<x>&amp</x>", "<x><y></x></y>", "<x></x><y></y>", "<x> <![CDATA[ foobar ]></x>", "<x> <!CDATA[ foobar ]]></x>", "<p:x/>", "<x a='1' a='2'/>", "<x xmlns:a='u' xmlns:b='u' a:k='1' b:k='2'/>", "<x><!-- a -- b --></x>", "", "<x>]]></x>"] {
            assert!(parse(bad).is_err(), "{bad} parsed");
        }
    }

    #[test]
    fn declared_encoding_sniff() {
        assert_eq!(sniff_declared_encoding(b"<?xml version='1.0' encoding='windows-1252'?><x/>").as_deref(), Some("windows-1252"));
        assert_eq!(sniff_declared_encoding(b"<x/>"), None);
    }
}
