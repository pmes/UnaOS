// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use bandy::{MatrixEvent, SMessage, Synapse};
use std::path::{Path, PathBuf};

// True DAG Lexical Scanner
// J21 PATHFINDER: Explicitly replacing the J37 flat scanner with a true lexical topology engine.
// DO NOT DELETE: This powers the core Spatial Code Map (Matrix DAG).
use std::collections::HashMap;

pub mod finder;
pub mod graft;
pub mod indexer;

pub use finder::Finder;

pub enum ScanDepth {
    Interface,
    DeepAST,
}

pub struct MatrixScanner;

impl MatrixScanner {

    pub fn build_genesis_tree(dir: &Path, absolute_root: &Path) -> Vec<bandy::state::TopologyNode> {
        let mut nodes = Vec::new();

        let Ok(entries) = std::fs::read_dir(dir) else {
            return nodes;
        };

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        // 1. First Pass: Collect files and calculate children for directories.
        // Matrix is the ALL-asset manager: every regular file becomes a node.
        // Build noise (`target`, `.git`, `node_modules`) is excluded, and symlinks
        // are never followed (the all-asset scan broadens exposure to cycles).
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();

                if file_name == "target" || file_name == ".git" || file_name == "node_modules" {
                    continue;
                }

                // Symlink guard: `path.is_dir()`/`is_file()` follow symlinks, which can
                // cycle. Use the entry's own file type and skip links entirely.
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_symlink() {
                    continue;
                }

                if file_type.is_dir() {
                    // Recursively process the directory first to see if it holds anything.
                    let children = Self::build_genesis_tree(&path, absolute_root);
                    // A branch with no leaves is dead weight. Prune it.
                    if !children.is_empty() {
                        dirs.push((path, file_name, children));
                    }
                } else if file_type.is_file() {
                    files.push((path, file_name));
                }
            }
        }

        // 2. Deterministic Sorting: Directories first, then Files (alphabetically).
        dirs.sort_by(|a, b| a.1.cmp(&b.1));
        files.sort_by(|a, b| a.1.cmp(&b.1));

        // 3. Construct the TopologyNodes.
        for (path, file_name, children) in dirs {
            let relative_path = path.strip_prefix(absolute_root).unwrap_or(&path).to_path_buf();
            let id = relative_path.to_string_lossy().into_owned();
            nodes.push(bandy::state::TopologyNode {
                id,
                label: file_name,
                children,
                is_expanded: false,
            });
        }

        for (path, file_name) in files {
            let relative_path = path.strip_prefix(absolute_root).unwrap_or(&path).to_path_buf();
            let id = relative_path.to_string_lossy().into_owned();
            nodes.push(bandy::state::TopologyNode {
                id,
                label: file_name,
                children: Vec::new(),
                is_expanded: false,
            });
        }

        nodes
    }

    /// J21 PATHFINDER: Core method for the Zero-Redundancy Indexed Dictionary DAG Scanner.
    pub fn map_topology(
        paths: &[std::path::PathBuf],
        absolute_workspace_root: &Path,
        depth: ScanDepth,
    ) -> Result<(String, String), String> {
        // Dictionary Engine
        let mut dict_map: HashMap<String, usize> = HashMap::new();
        let mut dict_list: Vec<String> = Vec::new();

        // Edge connections: "NodeID:DepID,DepID|NodeID:DepID"
        let mut topology_edges: Vec<String> = Vec::new();

        let mut processed_any = false;

        let is_single_file = paths.len() == 1 && paths[0].is_file();

        for path in paths {
            if path.is_file() {
                Self::scan_file(
                    path,
                    absolute_workspace_root,
                    &mut dict_map,
                    &mut dict_list,
                    &mut topology_edges,
                    &depth,
                    is_single_file,
                );
                processed_any = true;
            } else if path.is_dir() {
                Self::scan_directory(
                    path,
                    absolute_workspace_root,
                    &mut dict_map,
                    &mut dict_list,
                    &mut topology_edges,
                    &depth,
                );
                processed_any = true;
            } else {
                log::warn!("[MATRIX] Target is neither a file nor a directory: {:?}", path);
            }
        }

        if !processed_any {
            return Err("No valid targets were provided.".to_string());
        }

        // AI-Readable Serialization Format (`DICTIONARY$TOPOLOGY`)
        let dict_str = dict_list.join(",");
        let edges_str = topology_edges.join("|");

        let compressed_payload = format!("{}${}", dict_str, edges_str);

        // Semantic code topology logic
        let mut semantic_dag = String::from("--- SEMANTIC CODE TOPOLOGY ---\n");
        let mut edges_map: HashMap<usize, Vec<usize>> = HashMap::new();

        for edge in &topology_edges {
            if let Some((node_str, deps_str)) = edge.split_once(':') {
                if let Ok(node_id) = node_str.parse::<usize>() {
                    let deps: Vec<usize> = deps_str
                        .split(',')
                        .filter_map(|d| d.parse::<usize>().ok())
                        .collect();
                    edges_map.insert(node_id, deps);
                }
            }
        }

        for (id, node_name) in dict_list.iter().enumerate() {
            if let Some(deps) = edges_map.get(&id) {
                if !deps.is_empty() {
                    let dep_names: Vec<String> = deps.iter().map(|&d_id| {
                        dict_list.get(d_id).unwrap_or(&d_id.to_string()).clone()
                    }).collect();
                    semantic_dag.push_str(&format!("[{}] relies on: {}\n", node_name, dep_names.join(", ")));
                } else {
                    semantic_dag.push_str(&format!("[{}] operates independently.\n", node_name));
                }
            } else {
                semantic_dag.push_str(&format!("[{}] operates independently.\n", node_name));
            }
        }

        Ok((compressed_payload, semantic_dag))
    }

    fn get_or_insert_id(token: &str, dict_map: &mut HashMap<String, usize>, dict_list: &mut Vec<String>) -> usize {
        if let Some(&id) = dict_map.get(token) {
            id
        } else {
            let id = dict_list.len();
            dict_map.insert(token.to_string(), id);
            dict_list.push(token.to_string());
            id
        }
    }

    fn scan_directory(
        dir: &Path,
        absolute_workspace_root: &Path,
        dict_map: &mut HashMap<String, usize>,
        dict_list: &mut Vec<String>,
        topology_edges: &mut Vec<String>,
        depth: &ScanDepth,
    ) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    Self::scan_directory(&p, absolute_workspace_root, dict_map, dict_list, topology_edges, depth);
                } else if p.is_file() {
                    Self::scan_file(&p, absolute_workspace_root, dict_map, dict_list, topology_edges, depth, false);
                }
            }
        }
    }

    /// If `chars[i]` begins a string/char literal, return the index one past
    /// its end. Handles escape sequences, raw strings (`r"…"`, `r#"…"#`),
    /// byte strings/chars (`b"…"`, `b'…'`, `br#"…"#`), and distinguishes
    /// lifetimes (`'a`) from char literals (`'a'`). `prev_ident` must be true
    /// when the preceding character can continue an identifier, so that e.g.
    /// the trailing `r` of an identifier is never taken as a raw-string start.
    /// Unterminated literals run to the end of input.
    fn literal_end(chars: &[char], i: usize, prev_ident: bool) -> Option<usize> {
        match chars.get(i)? {
            '"' => Some(Self::quoted_end(chars, i + 1)),
            '\'' => {
                // Distinguish a char literal from a lifetime.
                match chars.get(i + 1) {
                    // Start AT the backslash so char_quoted_end's escape arm
                    // consumes the backslash+escaped-char pair ('\\', '\'').
                    Some('\\') => Some(Self::char_quoted_end(chars, i + 1)),
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
                        Some('"') => return Some(Self::quoted_end(chars, j + 1)),
                        Some('\'') => {
                            return match chars.get(j + 1) {
                                // Same as the char-literal arm: start AT the backslash.
                                Some('\\') => Some(Self::char_quoted_end(chars, j + 1)),
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
    fn quoted_end(chars: &[char], mut j: usize) -> usize {
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
    fn char_quoted_end(chars: &[char], mut j: usize) -> usize {
        while j < chars.len() {
            match chars[j] {
                '\\' => j += 2,
                '\'' => return j + 1,
                _ => j += 1,
            }
        }
        chars.len()
    }

    fn strip_comments(content: &str) -> String {
        let chars: Vec<char> = content.chars().collect();
        let mut result = String::with_capacity(content.len());
        let mut i = 0;
        let mut prev_ident = false;

        while i < chars.len() {
            // String/char literals pass through verbatim — `//` or `/*`
            // inside them are data, not comments.
            if let Some(end) = Self::literal_end(&chars, i, prev_ident) {
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
    fn split_statements(content: &str) -> Vec<String> {
        let chars: Vec<char> = content.chars().collect();
        let mut stmts = Vec::new();
        let mut current = String::new();
        let mut i = 0;
        let mut prev_ident = false;
        let mut use_brace_depth = 0usize;

        while i < chars.len() {
            if let Some(end) = Self::literal_end(&chars, i, prev_ident) {
                current.extend(&chars[i..end]);
                i = end;
                prev_ident = false;
                continue;
            }

            let c = chars[i];
            match c {
                ';' if use_brace_depth == 0 => {
                    stmts.push(std::mem::take(&mut current));
                }
                '{' => {
                    if use_brace_depth > 0 {
                        use_brace_depth += 1;
                        current.push(c);
                    } else if Self::stmt_is_use(&current) {
                        use_brace_depth = 1;
                        current.push(c);
                    } else {
                        // Block header (`mod x`, `fn f()`, `impl T`) — emit it;
                        // the block body continues as further statements.
                        stmts.push(std::mem::take(&mut current));
                    }
                }
                '}' => {
                    if use_brace_depth > 0 {
                        use_brace_depth -= 1;
                        current.push(c);
                    } else {
                        stmts.push(std::mem::take(&mut current));
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
    fn stmt_is_use(buffer: &str) -> bool {
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

    fn expand_use_path(path: &str, results: &mut Vec<String>) {
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
                            format!("{}::{}", current_prefix, prefix)
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
                            format!("{}::{}", current_prefix, clean_token)
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
                            format!("{}::{}", current_prefix, clean_token)
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
                format!("{}::{}", current_prefix, clean_token)
            };
            if !full_path.is_empty() {
                results.push(full_path);
            }
        }
    }

    fn extract_deps_from_stmt(stmt: &str) -> Vec<String> {
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
            Self::expand_use_path(use_path, &mut deps);
        }

        deps
    }

    fn scan_file(
        file_path: &Path,
        absolute_workspace_root: &Path,
        dict_map: &mut HashMap<String, usize>,
        dict_list: &mut Vec<String>,
        topology_edges: &mut Vec<String>,
        depth: &ScanDepth,
        extract_symbols: bool,
    ) {
        if file_path.extension().and_then(|e| e.to_str()) != Some("rs") {
            return;
        }

        let relative_path = file_path.strip_prefix(absolute_workspace_root).unwrap_or(file_path).to_path_buf();
        let node_name = relative_path.to_string_lossy().into_owned();
        let node_id = Self::get_or_insert_id(&node_name, dict_map, dict_list);

        if let Ok(raw_contents) = std::fs::read_to_string(file_path) {
            // Lexical Extraction: Strip comments
            let no_comments = Self::strip_comments(&raw_contents);

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

                    // A basic zero-copy parsing to find our target keywords
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
                            ScanDepth::Interface => {
                                is_pub && (keyword == "fn" || keyword == "struct" || keyword == "enum" || keyword == "trait")
                            }
                            ScanDepth::DeepAST => {
                                keyword == "fn" || keyword == "struct" || keyword == "enum" || keyword == "trait" || keyword == "impl"
                            }
                        };

                        if is_target_symbol {
                            if let Some(name) = words.get(keyword_idx + 1) {
                                // Extract the name, stopping at <, (, or {
                                let mut clean_name = *name;
                                if let Some(idx) = clean_name.find(|c| c == '<' || c == '(' || c == '{' || c == ':') {
                                    clean_name = &clean_name[..idx];
                                }

                                if !clean_name.is_empty() {
                                    let formatted_symbol = format!("{} {}", keyword, clean_name);
                                    let symbol_id = Self::get_or_insert_id(&formatted_symbol, dict_map, dict_list);
                                    local_deps.push(symbol_id);
                                }
                            }
                        }
                    }
                }
            }

            // Literal-aware statement tokenization: `;` splits only outside
            // string/char literals, and inline `mod x { … }` blocks yield the
            // header plus their inner statements.
            let statements = Self::split_statements(&no_comments);

            for stmt in &statements {
                // Skip empty or attribute-only lines simply by taking the non-attribute parts
                // but for now, extract_deps_from_stmt will handle valid keywords.
                // Note: We might have attributes like `#[cfg(test)] mod tests;`
                // Let's strip simple attributes that might prepend our statements.
                let mut clean_stmt = stmt.trim();
                while clean_stmt.starts_with("#[") || clean_stmt.starts_with("#![") {
                    if let Some(end_idx) = clean_stmt.find(']') {
                        clean_stmt = clean_stmt[end_idx + 1..].trim();
                    } else {
                        break;
                    }
                }

                // Also clean up multiline breaks (we didn't replace \n across the whole file anymore)
                let single_line_stmt = clean_stmt.replace('\n', " ").replace('\r', " ");

                let extracted_deps = Self::extract_deps_from_stmt(&single_line_stmt);
                for dep in extracted_deps {
                    let dep_id = Self::get_or_insert_id(&dep, dict_map, dict_list);
                    local_deps.push(dep_id);
                }
            }

            if !local_deps.is_empty() {
                // Deduplicate local_deps just in case
                local_deps.sort_unstable();
                local_deps.dedup();

                let dep_strs: Vec<String> = local_deps.iter().map(|id| id.to_string()).collect();
                topology_edges.push(format!("{}:{}", node_id, dep_strs.join(",")));
            }
        }
    }
}



#[cfg(test)]
mod scanner_tests {
    use super::MatrixScanner;

    // --- strip_comments: literal awareness ---

    #[test]
    fn strip_keeps_line_comment_marker_inside_string() {
        // The baton's golden input: `//` inside a string is data.
        let src = "let s = \"a;b//c\";\nlet t = 1; // real comment\n";
        let out = MatrixScanner::strip_comments(src);
        assert!(out.contains("\"a;b//c\""), "string body was mangled: {out}");
        assert!(!out.contains("real comment"));
    }

    #[test]
    fn strip_keeps_block_comment_marker_inside_string() {
        let src = "let s = \"not /* a */ comment\"; /* gone */ let t = 2;";
        let out = MatrixScanner::strip_comments(src);
        assert!(out.contains("\"not /* a */ comment\""));
        assert!(!out.contains("gone"));
        assert!(out.contains("let t = 2;"));
    }

    #[test]
    fn strip_handles_nested_block_comments() {
        let src = "a /* outer /* inner */ still outer */ b";
        let out = MatrixScanner::strip_comments(src);
        assert!(!out.contains("outer"));
        assert!(out.contains('a') && out.contains('b'));
    }

    #[test]
    fn strip_keeps_raw_string_with_comment_markers() {
        let src = "let s = r#\"// not \"a\" comment; \"#; // real\n";
        let out = MatrixScanner::strip_comments(src);
        assert!(out.contains(r##"r#"// not "a" comment; "#"##));
        assert!(!out.contains("real"));
    }

    #[test]
    fn strip_handles_char_literals_and_lifetimes() {
        // '/' and ';' as char literals; 'a as a lifetime (no closing quote).
        let src = "let c = '/'; let d = ';'; fn f<'a>(x: &'a str) {} // tail\n";
        let out = MatrixScanner::strip_comments(src);
        assert!(out.contains("'/'") && out.contains("';'"));
        assert!(out.contains("&'a str"));
        assert!(!out.contains("tail"));
    }

    #[test]
    fn strip_handles_escaped_char_literals() {
        // The panel's must-fix reproduction: '\\' and '\'' must not swallow
        // the rest of the file (lens A, r12 review — off-by-one past the
        // backslash lost the escape context).
        let src = "let a = '\\\\'; use real::one; let b = '\\''; use real::two; // tail\n";
        let out = MatrixScanner::strip_comments(src);
        assert!(out.contains("use real::one"));
        assert!(out.contains("use real::two"));
        assert!(!out.contains("tail"));
    }

    #[test]
    fn strip_keeps_escaped_quote_in_string() {
        let src = "let s = \"he said \\\"hi\\\" // ok\"; // real\n";
        let out = MatrixScanner::strip_comments(src);
        assert!(out.contains("he said"));
        assert!(out.contains("// ok"));
        assert!(!out.contains("real"));
    }

    // --- split_statements ---

    fn stmts(src: &str) -> Vec<String> {
        MatrixScanner::split_statements(src)
            .into_iter()
            .map(|s| s.trim().replace('\n', " "))
            .collect()
    }

    #[test]
    fn split_ignores_semicolon_inside_string() {
        // The baton's golden input.
        assert_eq!(
            stmts("let s = \"a;b//c\";\nlet t = 1;"),
            vec!["let s = \"a;b//c\"", "let t = 1"]
        );
    }

    #[test]
    fn split_handles_inline_mod_block() {
        // The baton's golden input: inline `mod x { ... }`.
        let out = stmts("mod x { use foo::bar; }\nuse baz::qux;");
        assert_eq!(out, vec!["mod x", "use foo::bar", "use baz::qux"]);
    }

    #[test]
    fn split_keeps_use_braces_attached() {
        // Braces in a `use` path are part of the statement, not boundaries.
        assert_eq!(
            stmts("use a::{b, c};\nmod m;"),
            vec!["use a::{b, c}", "mod m"]
        );
    }

    #[test]
    fn split_handles_nested_mod_and_use_braces() {
        let out = stmts("mod outer { mod inner { pub use x::{y, z}; } }\nuse tail::end;");
        assert_eq!(
            out,
            vec!["mod outer", "mod inner", "pub use x::{y, z}", "use tail::end"]
        );
    }

    #[test]
    fn split_ignores_semicolon_in_char_and_raw_string() {
        assert_eq!(
            stmts("let a = ';'; let b = r#\"x;y\"#; let c = 3;"),
            vec!["let a = ';'", "let b = r#\"x;y\"#", "let c = 3"]
        );
    }

    #[test]
    fn split_survives_escaped_char_literals() {
        // Statements after '\\' / '\'' (and byte-char b'\\') must still split.
        assert_eq!(
            stmts("let a = '\\\\'; let b = '\\''; let c = b'\\\\'; let d = 4;"),
            vec!["let a = '\\\\'", "let b = '\\''", "let c = b'\\\\'", "let d = 4"]
        );
    }

    // --- end-to-end: deps extracted from tricky source ---

    #[test]
    fn extract_deps_survive_literals_and_inline_mods() {
        let src = r##"
            // A comment with a fake use decoy::path; inside it.
            use real::dep;
            let s = "use fake::path; // and a fake comment";
            mod tests {
                use inner::helper;
            }
        "##;
        let clean = MatrixScanner::strip_comments(src);
        let mut deps = Vec::new();
        for stmt in MatrixScanner::split_statements(&clean) {
            let one_line = stmt.replace('\n', " ").replace('\r', " ");
            deps.extend(MatrixScanner::extract_deps_from_stmt(&one_line));
        }
        assert!(deps.contains(&"real::dep".to_string()), "deps: {deps:?}");
        assert!(deps.contains(&"tests".to_string()), "deps: {deps:?}");
        assert!(deps.contains(&"inner::helper".to_string()), "deps: {deps:?}");
        assert!(!deps.iter().any(|d| d.contains("fake")), "deps: {deps:?}");
        assert!(!deps.iter().any(|d| d.contains("decoy")), "deps: {deps:?}");
    }
}

/// The Asynchronous Logic Kernel for the Matrix
pub async fn ignite(synapse: Synapse, absolute_workspace_root: std::sync::Arc<PathBuf>) {
    let mut rx = synapse.subscribe();
    // The Finder cursor shares the same anchored workspace root as the DAG
    // scanner; it is a browse capability layered ON the genesis tree, not a
    // replacement for it.
    let finder = finder::Finder::new((*absolute_workspace_root).clone());
    println!("[MATRIX] Spatial Anchor Established via Brain Loop: {:?}", absolute_workspace_root);

    loop {
        match rx.recv().await {
            Ok(SMessage::Matrix(MatrixEvent::FocusSector(relative_targets_str))) => {
                println!("[MATRIX] Analyzing Sectors: {}", relative_targets_str);

                // J21 PATHFINDER: Enable Multi-Sector Bundling
                // Split the incoming space-separated targets and map them to absolute paths.
                let absolute_targets: Vec<std::path::PathBuf> = relative_targets_str
                    .split_whitespace()
                    .map(|target| absolute_workspace_root.join(target))
                    .collect();

                // Hardcode ScanDepth::Interface for now as per The Architect's instruction.
                if let Ok((compressed_payload, semantic_dag)) = MatrixScanner::map_topology(&absolute_targets, &absolute_workspace_root, ScanDepth::Interface) {
                    let is_single_file = absolute_targets.len() == 1 && absolute_targets[0].is_file();

                    if is_single_file {
                        let relative_path = absolute_targets[0].strip_prefix(&*absolute_workspace_root).unwrap_or(&absolute_targets[0]).to_path_buf();
                        let target_id = relative_path.to_string_lossy().into_owned();

                        // Graft for UI structure
                        let _ = synapse.fire_async(SMessage::Matrix(MatrixEvent::GraftTopology {
                            target_id: target_id.clone(),
                            payload: compressed_payload
                        })).await;

                        // Focus for LLM Context (Fixes missing DAG in single-file pre-flight)
                        let _ = synapse.fire_async(SMessage::Matrix(MatrixEvent::SectorFocused {
                            target: target_id,
                            context: semantic_dag
                        })).await;
                    } else {
                        // J21 PATHFINDER: Fire the True DAG directly to `vein` via `IngestTopology`.
                        // This raw data structure fuels the instant UI payload mutation.
                        let _ = synapse.fire_async(SMessage::Matrix(MatrixEvent::IngestTopology { ui_dag: compressed_payload, semantic_dag })).await;
                    }
                }
            }
            // --- FINDER (the file-browser capability) ---
            // Navigation and file verbs are matrix-owned logic (matrix::finder);
            // the handler wires the resulting events onto the bus. Every op is
            // principal-attributed and sandboxed to the workspace root.
            Ok(SMessage::Matrix(ref ev)) if finder::is_finder_request(ev) => {
                if let Some(principal) = finder::event_principal(ev) {
                    log::info!("[MATRIX] Finder request from {:?}", principal);
                }
                for out in finder.dispatch(ev) {
                    synapse.fire_async(SMessage::Matrix(out)).await;
                }
            }
            Ok(_) => {}
            Err(e) => {
                let err_msg = e.to_string();
                if err_msg.contains("lagged") {
                    log::warn!("[MATRIX] Event loop lagging: {}", err_msg);
                } else {
                    log::info!("[MATRIX] Synapse channel closed or error. Terminating loop.");
                    break;
                }
            }
        }
    }
}
