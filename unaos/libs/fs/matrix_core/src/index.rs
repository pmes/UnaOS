// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The token indexer — Matrix's lexical topology engine (J21 PATHFINDER), moved here from the host handler
//! (handlers/matrix/src/lib.rs `MatrixScanner`) so the image indexes with the same bytes.
//!
//! A Rust source is reduced to its dependency edges without a parser: comments are stripped literal-aware
//! ([`strip_comments`]), the rest is split into statements literal-aware ([`split_statements`]), and each
//! `mod x` / `use a::{b, c}` statement yields its paths ([`extract_deps_from_stmt`]). On a single-file scan the
//! symbol pass adds the file's `fn` / `struct` / `enum` / `trait` (and `impl` at [`ScanDepth::DeepAST`]).
//! Every token is interned once in a dictionary; the output is the `DICTIONARY$EDGES` payload
//! (`tok,tok,…$node:dep,dep|node:dep`) the vessels graft and the semantic DAG text the LLM context reads
//! ([`Indexer::finish`]).

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::{Ignore, Kind, Volume, join};

/// How deep the symbol pass reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanDepth {
    /// `pub` `fn` / `struct` / `enum` / `trait` only.
    Interface,
    /// Every `fn` / `struct` / `enum` / `trait` / `impl`.
    DeepAST,
}

/// Is `name` a Rust source by extension (`x.rs`; a bare `.rs` dotfile has no extension, as `Path` says)?
pub fn is_rust_source(name: &str) -> bool {
    let base = crate::base_name(name);
    match base.rsplit_once('.') {
        Some((stem, ext)) => !stem.is_empty() && ext == "rs",
        None => false,
    }
}

/// The dictionary and edges of one topology scan.
#[derive(Default)]
pub struct Indexer {
    dict_map: BTreeMap<String, usize>,
    dict_list: Vec<String>,
    /// `(node, deps)` in scan order, one per scanned file that has any dependency.
    edges: Vec<(usize, Vec<usize>)>,
}

impl Indexer {
    pub fn new() -> Indexer {
        Indexer::default()
    }

    /// The dictionary id of `token`, interned on first sight.
    pub fn intern(&mut self, token: &str) -> usize {
        if let Some(&id) = self.dict_map.get(token) {
            id
        } else {
            let id = self.dict_list.len();
            self.dict_map.insert(token.to_string(), id);
            self.dict_list.push(token.to_string());
            id
        }
    }

    /// The dictionary so far, in id order.
    pub fn dictionary(&self) -> &[String] {
        &self.dict_list
    }

    /// Scan one source already interned as `node_id`: strip comments, run the symbol pass when
    /// `extract_symbols` (the single-file scan), extract every `mod` / `use` dependency, and record the
    /// file's edge (deduplicated, ascending) when it has any.
    pub fn scan_source(&mut self, node_id: usize, raw_contents: &str, depth: ScanDepth, extract_symbols: bool) {
        // Lexical Extraction: Strip comments
        let no_comments = strip_comments(raw_contents);

        let mut local_deps = Vec::new();

        if extract_symbols {
            // Line-by-line lexical pass to find file symbols.
            for line in no_comments.lines() {
                let mut clean_line = line.trim();
                while clean_line.starts_with("#[") || clean_line.starts_with("#![") {
                    if let Some(end_idx) = clean_line.find(']') {
                        clean_line = clean_line[end_idx + 1..].trim();
                    } else {
                        break;
                    }
                }

                let words: Vec<&str> = clean_line.split_whitespace().collect();
                if words.is_empty() {
                    continue;
                }

                let mut is_pub = false;
                let mut keyword_idx = 0;

                if words[0] == "pub" {
                    is_pub = true;
                    keyword_idx = 1;
                    // Handle `pub (crate)` where there's a space
                    if words.len() > 1 && words[1].starts_with('(') {
                        keyword_idx = 2;
                    }
                } else if words[0].starts_with("pub(") {
                    is_pub = true;
                    keyword_idx = 1;
                }

                // Skip intermediate modifiers like `async`, `const`, `unsafe`, `extern`, `default`
                while keyword_idx < words.len() {
                    let w = words[keyword_idx];
                    if w == "async" || w == "const" || w == "unsafe" || w == "extern" || w == "default" {
                        keyword_idx += 1;
                    } else {
                        break;
                    }
                }

                if keyword_idx < words.len() {
                    let keyword = words[keyword_idx];

                    let is_target_symbol = match depth {
                        ScanDepth::Interface => is_pub && (keyword == "fn" || keyword == "struct" || keyword == "enum" || keyword == "trait"),
                        ScanDepth::DeepAST => keyword == "fn" || keyword == "struct" || keyword == "enum" || keyword == "trait" || keyword == "impl",
                    };

                    if is_target_symbol {
                        if let Some(name) = words.get(keyword_idx + 1) {
                            // Extract the name, stopping at <, (, or {
                            let mut clean_name = *name;
                            if let Some(idx) = clean_name.find(|c| c == '<' || c == '(' || c == '{' || c == ':') {
                                clean_name = &clean_name[..idx];
                            }

                            if !clean_name.is_empty() {
                                let formatted_symbol = alloc::format!("{} {}", keyword, clean_name);
                                let symbol_id = self.intern(&formatted_symbol);
                                local_deps.push(symbol_id);
                            }
                        }
                    }
                }
            }
        }

        // Literal-aware statement tokenization: `;` splits only outside string/char literals, and inline
        // `mod x { … }` blocks yield the header plus their inner statements.
        let statements = split_statements(&no_comments);

        for stmt in &statements {
            // Strip simple attributes that might prepend our statements (`#[cfg(test)] mod tests;`).
            let mut clean_stmt = stmt.trim();
            while clean_stmt.starts_with("#[") || clean_stmt.starts_with("#![") {
                if let Some(end_idx) = clean_stmt.find(']') {
                    clean_stmt = clean_stmt[end_idx + 1..].trim();
                } else {
                    break;
                }
            }

            let single_line_stmt = clean_stmt.replace('\n', " ").replace('\r', " ");

            for dep in extract_deps_from_stmt(&single_line_stmt) {
                let dep_id = self.intern(&dep);
                local_deps.push(dep_id);
            }
        }

        if !local_deps.is_empty() {
            local_deps.sort_unstable();
            local_deps.dedup();
            self.edges.push((node_id, local_deps));
        }
    }

    /// Intern `node_name` and scan `bytes` as its source; a source that is not UTF-8 is interned but not
    /// scanned (the host's `read_to_string` failing the same way).
    pub fn scan_file(&mut self, node_name: &str, bytes: Option<&[u8]>, depth: ScanDepth, extract_symbols: bool) {
        let id = self.intern(node_name);
        if let Some(text) = bytes.and_then(|b| core::str::from_utf8(b).ok()) {
            self.scan_source(id, text, depth, extract_symbols);
        }
    }

    /// The AI-readable `DICTIONARY$TOPOLOGY` payload and the semantic code topology text.
    pub fn finish(&self) -> (String, String) {
        let dict_str = self.dict_list.join(",");
        let edge_strs: Vec<String> = self
            .edges
            .iter()
            .map(|(n, deps)| {
                let d: Vec<String> = deps.iter().map(|id| id.to_string()).collect();
                alloc::format!("{}:{}", n, d.join(","))
            })
            .collect();
        let compressed_payload = alloc::format!("{}${}", dict_str, edge_strs.join("|"));

        // A node scanned twice keeps its LAST edge (the host's map insert, re-parsed from the edge strings).
        let mut edges_map: BTreeMap<usize, &Vec<usize>> = BTreeMap::new();
        for (n, deps) in &self.edges {
            edges_map.insert(*n, deps);
        }

        let mut semantic_dag = String::from("--- SEMANTIC CODE TOPOLOGY ---\n");
        for (id, node_name) in self.dict_list.iter().enumerate() {
            match edges_map.get(&id) {
                Some(deps) if !deps.is_empty() => {
                    let dep_names: Vec<String> = deps.iter().map(|&d| self.dict_list.get(d).cloned().unwrap_or_else(|| d.to_string())).collect();
                    semantic_dag.push_str(&alloc::format!("[{}] relies on: {}\n", node_name, dep_names.join(", ")));
                }
                _ => semantic_dag.push_str(&alloc::format!("[{}] operates independently.\n", node_name)),
            }
        }
        (compressed_payload, semantic_dag)
    }
}

/// Index every Rust source under `dir` on `vol` (no symbol pass): files named by `join(id_base, …)`, links
/// never followed, the ignore policy applied, children in the volume's own order. The image's walk (the host
/// keeps its `std::fs` walk in `MatrixScanner::map_topology` and feeds the same [`Indexer`]).
pub fn index_tree<V: Volume>(vol: &mut V, dir: &str, id_base: &str, ignore: &Ignore, depth: ScanDepth, ix: &mut Indexer) {
    let Ok(entries) = vol.list(dir) else {
        return;
    };
    for e in entries {
        if e.name.is_empty() || e.name == "." || e.name == ".." {
            continue;
        }
        let path = join(dir, &e.name);
        if ignore.ignored(&path, &e.name, e.kind == Kind::Dir) {
            continue;
        }
        let id = join(id_base, &e.name);
        match e.kind {
            Kind::Dir => index_tree(vol, &path, &id, ignore, depth, ix),
            Kind::File if is_rust_source(&e.name) => {
                let bytes = vol.read(&path);
                ix.scan_file(&id, bytes.as_deref(), depth, false);
            }
            _ => {}
        }
    }
}

// ---- the lexer (moved verbatim from the host MatrixScanner; `Self::` dropped) -------------------------------


/// If `chars[i]` begins a string/char literal, return the index one past
/// its end. Handles escape sequences, raw strings (`r"…"`, `r#"…"#`),
/// byte strings/chars (`b"…"`, `b'…'`, `br#"…"#`), and distinguishes
/// lifetimes (`'a`) from char literals (`'a'`). `prev_ident` must be true
/// when the preceding character can continue an identifier, so that e.g.
/// the trailing `r` of an identifier is never taken as a raw-string start.
/// Unterminated literals run to the end of input.
pub fn literal_end(chars: &[char], i: usize, prev_ident: bool) -> Option<usize> {
    match chars.get(i)? {
        '"' => Some(quoted_end(chars, i + 1)),
        '\'' => {
            // Distinguish a char literal from a lifetime.
            match chars.get(i + 1) {
                // Start AT the backslash so char_quoted_end's escape arm
                // consumes the backslash+escaped-char pair ('\\', '\'').
                Some('\\') => Some(char_quoted_end(chars, i + 1)),
                Some(_) if chars.get(i + 2) == Some(&'\'') => Some(i + 3),
                _ => None, // lifetime (`'a`, `'static`) — not a literal
            }
        }
        'r' | 'b' if !prev_ident => {
            // Raw / byte string starts: r"…", r#"…"#, b"…", b'…', br#"…"#
            let mut j = i;
            if chars[j] == 'b' {
                j += 1;
                match chars.get(j) {
                    Some('"') => return Some(quoted_end(chars, j + 1)),
                    Some('\'') => {
                        return match chars.get(j + 1) {
                            // Same as the char-literal arm: start AT the backslash.
                            Some('\\') => Some(char_quoted_end(chars, j + 1)),
                            Some(_) if chars.get(j + 2) == Some(&'\'') => Some(j + 3),
                            _ => None,
                        };
                    }
                    Some('r') => {} // fall through to raw-string handling
                    _ => return None,
                }
            }
            if chars.get(j) != Some(&'r') {
                return None;
            }
            j += 1;
            let mut hashes = 0;
            while chars.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if chars.get(j) != Some(&'"') {
                return None;
            }
            j += 1;
            // Scan for the closing `"` followed by `hashes` hashes.
            while j < chars.len() {
                if chars[j] == '"' && chars[j + 1..].iter().take(hashes).filter(|&&c| c == '#').count() == hashes {
                    return Some(j + 1 + hashes);
                }
                j += 1;
            }
            Some(chars.len())
        }
        _ => None,
    }
}

/// End of a `"`-delimited body starting at `j` (past the opening quote),
/// honoring `\` escapes. Returns the index one past the closing quote.
pub fn quoted_end(chars: &[char], mut j: usize) -> usize {
    while j < chars.len() {
        match chars[j] {
            '\\' => j += 2,
            '"' => return j + 1,
            _ => j += 1,
        }
    }
    chars.len()
}

/// End of a `'`-delimited body starting at `j`, honoring `\` escapes.
pub fn char_quoted_end(chars: &[char], mut j: usize) -> usize {
    while j < chars.len() {
        match chars[j] {
            '\\' => j += 2,
            '\'' => return j + 1,
            _ => j += 1,
        }
    }
    chars.len()
}

pub fn strip_comments(content: &str) -> String {
    let chars: Vec<char> = content.chars().collect();
    let mut result = String::with_capacity(content.len());
    let mut i = 0;
    let mut prev_ident = false;

    while i < chars.len() {
        // String/char literals pass through verbatim — `//` or `/*`
        // inside them are data, not comments.
        if let Some(end) = literal_end(&chars, i, prev_ident) {
            result.extend(&chars[i..end]);
            i = end;
            prev_ident = false;
            continue;
        }

        let c = chars[i];
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            // Line comment: drop to (but keep) the newline.
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            prev_ident = false;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            // Block comment — Rust block comments nest.
            let mut depth = 1;
            i += 2;
            while i < chars.len() && depth > 0 {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            // Replace with a space so adjacent tokens don't glue together.
            result.push(' ');
            prev_ident = false;
            continue;
        }

        result.push(c);
        prev_ident = c.is_alphanumeric() || c == '_';
        i += 1;
    }

    result
}

/// Split comment-free source into statements, literal-aware. `;` splits
/// only outside string/char literals. `{` / `}` also act as statement
/// boundaries (so `mod x { use y; }` yields `mod x` and `use y`) — except
/// inside a `use` statement, where braces are part of the import path and
/// stay attached (`use a::{b, c}`).
pub fn split_statements(content: &str) -> Vec<String> {
    let chars: Vec<char> = content.chars().collect();
    let mut stmts = Vec::new();
    let mut current = String::new();
    let mut i = 0;
    let mut prev_ident = false;
    let mut use_brace_depth = 0usize;

    while i < chars.len() {
        if let Some(end) = literal_end(&chars, i, prev_ident) {
            current.extend(&chars[i..end]);
            i = end;
            prev_ident = false;
            continue;
        }

        let c = chars[i];
        match c {
            ';' if use_brace_depth == 0 => {
                stmts.push(core::mem::take(&mut current));
            }
            '{' => {
                if use_brace_depth > 0 {
                    use_brace_depth += 1;
                    current.push(c);
                } else if stmt_is_use(&current) {
                    use_brace_depth = 1;
                    current.push(c);
                } else {
                    // Block header (`mod x`, `fn f()`, `impl T`) — emit it;
                    // the block body continues as further statements.
                    stmts.push(core::mem::take(&mut current));
                }
            }
            '}' => {
                if use_brace_depth > 0 {
                    use_brace_depth -= 1;
                    current.push(c);
                } else {
                    stmts.push(core::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
        prev_ident = c.is_alphanumeric() || c == '_';
        i += 1;
    }
    stmts.push(current);

    stmts.retain(|s| !s.trim().is_empty());
    stmts
}

/// Does this (possibly attribute/visibility-prefixed) statement buffer
/// begin a `use` declaration?
pub fn stmt_is_use(buffer: &str) -> bool {
    let mut s = buffer.trim_start();
    while s.starts_with("#[") || s.starts_with("#![") {
        match s.find(']') {
            Some(end) => s = s[end + 1..].trim_start(),
            None => return false,
        }
    }
    if let Some(rest) = s.strip_prefix("pub") {
        s = rest.trim_start();
        if s.starts_with('(') {
            match s.find(')') {
                Some(end) => s = s[end + 1..].trim_start(),
                None => return false,
            }
        }
    }
    s == "use" || s.starts_with("use ") || s.starts_with("use\n") || s.starts_with("use\t")
}

pub fn expand_use_path(path: &str, results: &mut Vec<String>) {
    let path = path.trim();
    if path.is_empty() {
        return;
    }

    // Fast path for simple non-bracketed imports.
    if !path.contains('{') {
        // Remove " as alias" if present.
        let clean_path = if let Some(as_idx) = path.find(" as ") {
            path[..as_idx].trim()
        } else {
            path
        };
        if !clean_path.is_empty() {
            results.push(clean_path.to_string());
        }
        return;
    }

    let mut stack = Vec::new();
    let mut current_prefix = String::new();
    let mut current_token = String::new();

    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                let prefix = current_token.trim().trim_end_matches("::").trim();
                if !prefix.is_empty() {
                    let full_prefix = if current_prefix.is_empty() {
                        prefix.to_string()
                    } else {
                        alloc::format!("{}::{}", current_prefix, prefix)
                    };
                    stack.push(current_prefix.clone());
                    current_prefix = full_prefix;
                } else {
                    stack.push(current_prefix.clone());
                }
                current_token.clear();
            }
            '}' => {
                let token = current_token.trim();
                if !token.is_empty() {
                    let clean_token = if let Some(as_idx) = token.find(" as ") {
                        token[..as_idx].trim()
                    } else {
                        token
                    };

                    let full_path = if clean_token == "self" {
                        current_prefix.clone()
                    } else if current_prefix.is_empty() {
                        clean_token.to_string()
                    } else {
                        alloc::format!("{}::{}", current_prefix, clean_token)
                    };
                    if !full_path.is_empty() {
                        results.push(full_path);
                    }
                }

                if let Some(prev_prefix) = stack.pop() {
                    current_prefix = prev_prefix;
                }
                current_token.clear();
            }
            ',' => {
                let token = current_token.trim();
                if !token.is_empty() {
                    let clean_token = if let Some(as_idx) = token.find(" as ") {
                        token[..as_idx].trim()
                    } else {
                        token
                    };

                    let full_path = if clean_token == "self" {
                        current_prefix.clone()
                    } else if current_prefix.is_empty() {
                        clean_token.to_string()
                    } else {
                        alloc::format!("{}::{}", current_prefix, clean_token)
                    };
                    if !full_path.is_empty() {
                        results.push(full_path);
                    }
                }
                current_token.clear();
            }
            _ => {
                current_token.push(c);
            }
        }
    }

    // Handle any trailing token (though unusual in well-formed bracketed uses)
    let token = current_token.trim();
    if !token.is_empty() {
        let clean_token = if let Some(as_idx) = token.find(" as ") {
            token[..as_idx].trim()
        } else {
            token
        };

        let full_path = if clean_token == "self" {
            current_prefix.clone()
        } else if current_prefix.is_empty() {
            clean_token.to_string()
        } else {
            alloc::format!("{}::{}", current_prefix, clean_token)
        };
        if !full_path.is_empty() {
            results.push(full_path);
        }
    }
}

pub fn extract_deps_from_stmt(stmt: &str) -> Vec<String> {
    let mut deps = Vec::new();
    let stmt = stmt.trim();

    // 1. Handle visibility modifiers
    let mut content = stmt;
    if content.starts_with("pub") {
        content = &content[3..].trim_start();
        if content.starts_with('(') {
            // Skip past the matching closing parenthesis
            let mut depth = 0;
            let mut end_idx = 0;
            for (i, c) in content.char_indices() {
                if c == '(' {
                    depth += 1;
                } else if c == ')' {
                    depth -= 1;
                    if depth == 0 {
                        end_idx = i;
                        break;
                    }
                }
            }
            if end_idx > 0 {
                content = &content[end_idx + 1..].trim_start();
            }
        }
    }

    // 2. Parse mod or use
    if content.starts_with("mod ") {
        let mod_name = content[4..].trim();
        // In cases like `mod a { ... }`, we only care about the name before `{` if any,
        // though our split(';') might not give us blocks perfectly.
        // We assume standard `mod a;` since blocks wouldn't end in `;` without internal `;`.
        // Let's take the first token.
        let name = mod_name.split_whitespace().next().unwrap_or("").trim_end_matches('{').trim();
        if !name.is_empty() {
            deps.push(name.to_string());
        }
    } else if content.starts_with("use ") {
        let use_path = content[4..].trim();
        expand_use_path(use_path, &mut deps);
    }

    deps
}
