//! The Sources panel (AETHERDEVTOOLS3, LEDGER SR128): the page's scripts by URL (inline scripts numbered in
//! document order), the selected script's source with gutter line numbers, and the current line marked —
//! the line a Console frame (`    at fn (url:line:col)`) names when it is followed ([`Sources::goto`]).
//!
//! Seam: the scripts are the document's `<script>` elements of a JavaScript type, read from the live DOM
//! (`AetherEngine::document`) in tree order; an external script's source is what the engine ran or
//! pre-fetched under its absolute URL (`js::ran_source`: PageState.ran_sources, then
//! PageState.script_sources), an inline script's is its element's text. The file name a frame carries is the
//! script's absolute URL for `<script src>` and the document's URL for an inline script (JSPOS, SR127:
//! `js/mod.rs` `script_file`), with lines counted from the script element's own text — so a frame naming
//! the document names an inline script, not which one. [`resolve`] picks the inline script the engine ran
//! last under that name when its text covers the line, else the first inline script that does.
//!
//! The pane's own state is the selected script and the current line ([`Sources`]); nothing is cached.

use crate::AetherEngine;

/// Source lines the view shows at most, centred on the current line.
pub const VIEW_LINES: usize = 400;

/// One script of the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    /// What the list shows: the absolute URL, or `<document url> (inline #n)`.
    pub name: String,
    /// The file name the script's frames carry (its URL, or the document's URL for an inline script).
    pub file: String,
    /// The inline script's number in document order (1-based); None for `<script src>`.
    pub inline: Option<usize>,
    /// The element's arena id.
    pub node: usize,
    /// The source text, when the engine holds it.
    pub source: Option<String>,
}

/// The panel's state: the selected script (an index into [`scripts`]) and the current line (1-based).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sources {
    pub selected: Option<usize>,
    pub line: Option<u32>,
}

/// `type` values that make a `<script>` run as JavaScript (HTML §4.12.1: empty, absent, a JavaScript MIME
/// type essence, or `module`).
fn is_script_type(t: Option<&str>) -> bool {
    let Some(t) = t else { return true };
    let t = t.trim().to_ascii_lowercase();
    t.is_empty()
        || t == "module"
        || matches!(
            t.split(';').next().unwrap_or("").trim(),
            "text/javascript"
                | "application/javascript"
                | "application/ecmascript"
                | "application/x-ecmascript"
                | "application/x-javascript"
                | "text/ecmascript"
                | "text/javascript1.0"
                | "text/javascript1.1"
                | "text/javascript1.2"
                | "text/javascript1.3"
                | "text/javascript1.4"
                | "text/javascript1.5"
                | "text/jscript"
                | "text/livescript"
                | "text/x-ecmascript"
                | "text/x-javascript"
        )
}

/// A URL without its fragment (frames and the list compare on this).
fn bare(u: &str) -> &str {
    u.split('#').next().unwrap_or(u)
}

/// The document's scripts in tree order.
pub fn scripts(engine: &AetherEngine) -> Vec<Script> {
    let mut out = Vec::new();
    let Some(doc) = engine.document.as_ref() else { return out };
    let page = engine.current_url();
    let mut inline = 0usize;
    for n in doc.document_node().descendants() {
        let Some(e) = n.as_element() else { continue };
        if e.name.local.0 != "script" {
            continue;
        }
        let (ty, src) = {
            let attrs = e.attributes.borrow();
            (attrs.get("type").map(str::to_string), attrs.get("src").map(str::to_string))
        };
        if !is_script_type(ty.as_deref()) {
            continue;
        }
        match src {
            Some(raw) => {
                let Ok(u) = html_core::url::Url::parse_str_with_base(raw.trim(), Some(&page)) else { continue };
                let url = u.serialize(true);
                let source = crate::js::ran_source(&url).map(|s| s.to_string());
                out.push(Script { name: url.clone(), file: url, inline: None, node: n.id().0, source });
            }
            None => {
                inline += 1;
                out.push(Script {
                    name: format!("{page} (inline #{inline})"),
                    file: page.clone(),
                    inline: Some(inline),
                    node: n.id().0,
                    source: Some(n.text_contents()),
                });
            }
        }
    }
    out
}

/// `src` split into lines where js_core's line table splits them (LF, CR, CR LF once, U+2028, U+2029).
pub fn split_lines(src: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut it = src.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
            lines.push(&src[start..i]);
            start = i + c.len_utf8();
            if c == '\r' && it.peek().map(|&(_, n)| n) == Some('\n') {
                it.next();
                start += 1;
            }
        }
    }
    lines.push(&src[start..]);
    lines
}

/// The target a Console line names: `(file, line, col)` of a V8-shaped frame (`at name (file:line:col)`,
/// `at file:line:col`, `at new Name (file:line:col)`), wherever it sits in the line (a face's gutter glyph
/// may precede it). None for a frame with no line (`at name (file)`) or any other text.
pub fn frame_target(text: &str) -> Option<(String, u32, u32)> {
    let t = text.trim();
    let i = t.find("at ").filter(|&i| i == 0 || t[..i].ends_with(char::is_whitespace))?;
    let rest = t[i + 3..].trim_end();
    let loc = match (rest.strip_suffix(')'), rest.rfind(" (")) {
        (Some(r), Some(k)) => &r[k + 2..],
        _ => rest,
    };
    let mut it = loc.rsplitn(3, ':');
    let col = it.next()?.parse().ok()?;
    let line = it.next()?.parse().ok()?;
    let file = it.next().filter(|f| !f.is_empty() && !f.contains(char::is_whitespace))?;
    Some((file.to_string(), line, col))
}

/// The index (into [`scripts`]) of the script a frame naming `file` at `line` is in: the external script of
/// that URL; else, among the inline scripts (whose frames name the document), the one the engine ran last
/// under that name when its text covers `line`, else the first inline script that covers it, else the first.
pub fn resolve(engine: &AetherEngine, file: &str, line: u32) -> Option<usize> {
    let all = scripts(engine);
    let want = bare(file);
    if let Some(i) = all.iter().position(|s| s.inline.is_none() && bare(&s.file) == want) {
        return Some(i);
    }
    let inline: Vec<usize> = all.iter().enumerate().filter(|(_, s)| s.inline.is_some() && bare(&s.file) == want).map(|(i, _)| i).collect();
    let covers = |i: &usize| all[*i].source.as_deref().is_some_and(|s| split_lines(s).len() >= line as usize);
    let last_ran = crate::js::ran_source(file);
    inline
        .iter()
        .copied()
        .find(|i| covers(i) && last_ran.as_deref().is_some_and(|r| all[*i].source.as_deref() == Some(r)))
        .or_else(|| inline.iter().copied().find(covers))
        .or_else(|| inline.first().copied())
}

/// The numbered source view: `{mark} {n:>w} | {text}`, the current line marked `>`, at most [`VIEW_LINES`]
/// lines centred on it.
pub fn view(source: &str, current: Option<u32>) -> Vec<String> {
    let lines = split_lines(source);
    let w = lines.len().to_string().len();
    let cur = current.map(|c| c as usize).filter(|c| *c >= 1 && *c <= lines.len());
    let first = cur.map(|c| c.saturating_sub(VIEW_LINES / 2).max(1)).unwrap_or(1);
    let last = (first + VIEW_LINES - 1).min(lines.len());
    (first..=last)
        .map(|n| {
            let mark = if Some(n) == cur { '>' } else { ' ' };
            format!("{mark} {n:>w$} | {}", lines[n - 1])
        })
        .collect()
}

impl Sources {
    /// Shows the script a frame naming `file` at `line` is in, with `line` current. False (nothing changes)
    /// when the document has no such script.
    pub fn goto(&mut self, engine: &AetherEngine, file: &str, line: u32) -> bool {
        match resolve(engine, file, line) {
            Some(i) => {
                self.selected = Some(i);
                self.line = Some(line);
                true
            }
            None => false,
        }
    }

    /// The panel as text lines: the script list (the selected one marked `>`), then the selected script's
    /// name and its numbered source.
    pub fn lines(&self, engine: &AetherEngine) -> Vec<String> {
        let all = scripts(engine);
        let mut out = vec![format!("Scripts ({})", all.len())];
        let w = all.len().to_string().len();
        for (i, s) in all.iter().enumerate() {
            let mark = if Some(i) == self.selected { '>' } else { ' ' };
            out.push(format!("{mark} {:>w$}  {}", i + 1, s.name));
        }
        let Some(s) = self.selected.and_then(|i| all.get(i)) else { return out };
        out.push(String::new());
        out.push(match self.line {
            Some(l) => format!("{}:{l}", s.name),
            None => s.name.clone(),
        });
        match &s.source {
            Some(src) => out.extend(view(src, self.line)),
            None => out.push("  (source not held: the engine did not run or pre-fetch this script)".to_string()),
        }
        out
    }
}
