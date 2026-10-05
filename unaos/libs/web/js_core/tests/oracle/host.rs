//! The oracle's tiny test host: parses a (simple, well-formed) HTML page, builds the DOM of `dom.js` in js_core,
//! runs the inline scripts in document order, drains the event loop and serializes the DOM — the same view
//! Chromium's `--dump-dom` gives after load. Not Aether: just enough DOM for the oracle pages.

use js_core::vm::*;
use std::cell::RefCell;
use std::rc::Rc;

pub enum HNode {
    Text(String),
    El(String, Vec<(String, String)>, Vec<HNode>),
}

const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"];

fn decode_entities(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').unwrap_or(0);
        let ent = &rest[..end.max(1)];
        let rep = match ent {
            "&amp" => Some('&'),
            "&lt" => Some('<'),
            "&gt" => Some('>'),
            "&quot" => Some('"'),
            "&nbsp" => Some('\u{a0}'),
            _ if ent.starts_with("&#x") => u32::from_str_radix(&ent[3..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with("&#") => ent[2..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match rep {
            Some(c) if end > 0 => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parse a well-formed page (explicit html/head/body, quoted attributes, raw-text script/style).
pub fn parse_html(src: &str) -> (HNode, String) {
    let b = src.as_bytes();
    let mut i = 0;
    let mut stack: Vec<(String, Vec<(String, String)>, Vec<HNode>)> = vec![(String::from("#root"), Vec::new(), Vec::new())];
    let mut trailing = String::new();
    while i < b.len() {
        if b[i] == b'<' {
            if src[i..].starts_with("<!--") {
                i = src[i..].find("-->").map(|e| i + e + 3).unwrap_or(b.len());
                continue;
            }
            if src[i..].starts_with("<!") {
                i = src[i..].find('>').map(|e| i + e + 1).unwrap_or(b.len());
                continue;
            }
            if src[i..].starts_with("</") {
                let e = src[i..].find('>').unwrap() + i;
                let name = src[i + 2..e].trim().to_ascii_lowercase();
                i = e + 1;
                while stack.len() > 1 {
                    let (n, a, c) = stack.pop().unwrap();
                    let done = n == name;
                    stack.last_mut().unwrap().2.push(HNode::El(n, a, c));
                    if done {
                        break;
                    }
                }
                continue;
            }
            // start tag
            let mut j = i + 1;
            while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' && b[j] != b'/' {
                j += 1;
            }
            let name = src[i + 1..j].to_ascii_lowercase();
            let mut attrs = Vec::new();
            loop {
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if b[j] == b'>' || b[j] == b'/' {
                    break;
                }
                let s = j;
                while j < b.len() && b[j] != b'=' && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                    j += 1;
                }
                let an = src[s..j].to_ascii_lowercase();
                let mut av = String::new();
                if b[j] == b'=' {
                    j += 1;
                    let q = b[j];
                    if q == b'"' || q == b'\'' {
                        let e = src[j + 1..].find(q as char).unwrap() + j + 1;
                        av = decode_entities(&src[j + 1..e]);
                        j = e + 1;
                    } else {
                        let s2 = j;
                        while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                            j += 1;
                        }
                        av = decode_entities(&src[s2..j]);
                    }
                }
                attrs.push((an, av));
            }
            let e = src[j..].find('>').unwrap() + j;
            i = e + 1;
            if VOID.contains(&name.as_str()) {
                stack.last_mut().unwrap().2.push(HNode::El(name, attrs, Vec::new()));
                continue;
            }
            if name == "script" || name == "style" {
                let close = format!("</{}", name);
                let end = src[i..].find(&close).map(|e| e + i).unwrap_or(b.len());
                let text = src[i..end].to_string();
                i = src[end..].find('>').map(|e| end + e + 1).unwrap_or(b.len());
                let kids = if text.is_empty() { Vec::new() } else { vec![HNode::Text(text)] };
                stack.last_mut().unwrap().2.push(HNode::El(name, attrs, kids));
                continue;
            }
            stack.push((name, attrs, Vec::new()));
        } else {
            let e = src[i..].find('<').map(|e| e + i).unwrap_or(b.len());
            let t = decode_entities(&src[i..e]);
            i = e;
            if !t.is_empty() && stack.len() > 1 {
                stack.last_mut().unwrap().2.push(HNode::Text(t));
            } else if !t.is_empty() {
                // Whitespace after </html> is processed "in body" and lands at the end of <body>.
                trailing.push_str(&t);
            }
        }
    }
    while stack.len() > 1 {
        let (n, a, c) = stack.pop().unwrap();
        stack.last_mut().unwrap().2.push(HNode::El(n, a, c));
    }
    let root = stack.pop().unwrap();
    let html = root.2.into_iter().find(|n| matches!(n, HNode::El(..))).expect("no root element");
    (html, trailing)
}

fn js_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\u{2028}' => o.push_str("\\u2028"),
            '\u{2029}' => o.push_str("\\u2029"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn tree_js(n: &HNode, out: &mut String, scripts: &mut Vec<String>) {
    match n {
        HNode::Text(t) => out.push_str(&js_str(t)),
        HNode::El(name, attrs, kids) => {
            if name == "script" {
                if let Some(HNode::Text(t)) = kids.first() {
                    scripts.push(t.clone());
                }
            }
            out.push_str(&format!("[{},[", js_str(name)));
            for (k, (a, v)) in attrs.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                out.push_str(&format!("[{},{}]", js_str(a), js_str(v)));
            }
            out.push_str("],[");
            for (k, c) in kids.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                tree_js(c, out, scripts);
            }
            out.push_str("]]");
        }
    }
}

struct OracleHost {
    out: Rc<RefCell<String>>,
}

impl Host for OracleHost {
    fn console(&mut self, _level: u8, msg: &str) {
        let mut o = self.out.borrow_mut();
        o.push_str(msg);
        o.push('\n');
    }
}

/// Run a page; returns (serialized DOM, console output).
pub fn run_page(html: &str) -> (String, String) {
    let (tree, trailing) = parse_html(html);
    let mut js = String::from("__domBuild(");
    let mut scripts = Vec::new();
    tree_js(&tree, &mut js, &mut scripts);
    js.push_str(");");
    let out = Rc::new(RefCell::new(String::new()));
    let mut vm = Vm::new(Box::new(OracleHost { out: out.clone() }));
    js_core::builtins::host::install_host_globals(&mut vm);
    vm.budget = Some(2_000_000_000);
    vm.run_script_str(include_str!("dom.js")).expect("dom.js");
    vm.run_script_str(&js).expect("dom build");
    for s in &scripts {
        // Each classic script runs, then a microtask checkpoint (HTML "clean up after running script").
        let units: Vec<u16> = s.encode_utf16().collect();
        if let Err(e) = vm.run_script(&units) {
            let m = vm.error_string(&e);
            out.borrow_mut().push_str(&format!("Uncaught {}\n", m));
        }
        let _ = vm.checkpoint();
    }
    // The parser reaches the end of the document after the (trailing) scripts ran: whitespace after </html>
    // is inserted at the end of <body> then.
    if !trailing.is_empty() {
        let code = format!("(function(b,t){{var l=b.lastChild; if(l&&l.nodeType===3) l.data+=t; else b.appendChild(document.createTextNode(t));}})(document.body,{});", js_str(&trailing));
        vm.run_script_str(&code).expect("trailing text");
    }
    let _ = vm.run_event_loop(100_000);
    let dump = match vm.run_script_str("__domDump()") {
        Ok(Value::String(s)) => s.to_rust(),
        _ => String::from("<dump failed>"),
    };
    let console = out.borrow().clone();
    (dump, console)
}
