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
// MATRIXCORE (SR99, R104): the engine itself — the genesis rules, the lexer, the dictionary and the semantic DAG —
// lives in `matrix_core` (unaos/libs/fs/matrix_core, no_std), the bytes UNA.ELF links too. This handler is its
// host face: the std volume (volume.rs), the walk over host paths, and the bus loop below.

pub mod finder;
pub mod graft;
pub mod indexer;
pub mod trash; // TRASHTIME M3 (B308): the Trash on a UnaFS vault, by attributes
pub mod volume; // MATRIXCORE (SR99): the core's Volume over std::fs

pub use finder::Finder;
pub use matrix_core;
pub use matrix_core::ScanDepth;
pub use volume::StdVolume;

pub struct MatrixScanner;

/// A core genesis node as the bus's `TopologyNode` (collapsed, as the host always built it).
fn topology(n: matrix_core::Node) -> bandy::state::TopologyNode {
    bandy::state::TopologyNode {
        id: n.id,
        label: n.label,
        children: n.children.into_iter().map(topology).collect(),
        is_expanded: false,
    }
}

impl MatrixScanner {

    /// The genesis tree of `dir`, ids relative to `absolute_root` — `matrix_core::genesis_ids` over the std
    /// volume with the build-noise policy (`target`, `.git`, `node_modules`), which is the rule set this
    /// function always had: every regular file a node, symlinks never followed, empty branches pruned, dirs
    /// first then files in byte order. A `dir` outside `absolute_root` keeps its absolute path as the id base
    /// (the old `strip_prefix(..).unwrap_or(path)`).
    pub fn build_genesis_tree(dir: &Path, absolute_root: &Path) -> Vec<bandy::state::TopologyNode> {
        let ignore = matrix_core::Ignore::build_noise();
        let nodes = match dir.strip_prefix(absolute_root) {
            Ok(rel) => {
                let rel = rel.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
                matrix_core::genesis(&mut StdVolume::new(absolute_root), &rel, &ignore)
            }
            Err(_) => {
                let base = dir.to_string_lossy().into_owned();
                matrix_core::genesis_ids(&mut StdVolume::new(dir), "", &base, &ignore)
            }
        };
        nodes.into_iter().map(topology).collect()
    }

    /// The core's query model over the genesis tree of `absolute_root` (see `matrix_core::query`).
    pub fn find(absolute_root: &Path, q: &str) -> Vec<matrix_core::Hit> {
        let tree = matrix_core::genesis(&mut StdVolume::new(absolute_root), "", &matrix_core::Ignore::build_noise());
        matrix_core::query(&tree, q)
    }

    /// J21 PATHFINDER: Core method for the Zero-Redundancy Indexed Dictionary DAG Scanner. The token work is
    /// `matrix_core::Indexer`'s; the host walk (`is_file` / `is_dir` / `read_dir` order) is unchanged.
    pub fn map_topology(
        paths: &[std::path::PathBuf],
        absolute_workspace_root: &Path,
        depth: ScanDepth,
    ) -> Result<(String, String), String> {
        let mut ix = matrix_core::Indexer::new();
        let mut processed_any = false;

        let is_single_file = paths.len() == 1 && paths[0].is_file();

        for path in paths {
            if path.is_file() {
                Self::scan_file(path, absolute_workspace_root, &mut ix, depth, is_single_file);
                processed_any = true;
            } else if path.is_dir() {
                Self::scan_directory(path, absolute_workspace_root, &mut ix, depth);
                processed_any = true;
            } else {
                log::warn!("[MATRIX] Target is neither a file nor a directory: {:?}", path);
            }
        }

        if !processed_any {
            return Err("No valid targets were provided.".to_string());
        }

        // AI-Readable Serialization Format (`DICTIONARY$TOPOLOGY`) + the semantic code topology.
        Ok(ix.finish())
    }

    fn scan_directory(dir: &Path, absolute_workspace_root: &Path, ix: &mut matrix_core::Indexer, depth: ScanDepth) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    Self::scan_directory(&p, absolute_workspace_root, ix, depth);
                } else if p.is_file() {
                    Self::scan_file(&p, absolute_workspace_root, ix, depth, false);
                }
            }
        }
    }

    fn scan_file(file_path: &Path, absolute_workspace_root: &Path, ix: &mut matrix_core::Indexer, depth: ScanDepth, extract_symbols: bool) {
        if file_path.extension().and_then(|e| e.to_str()) != Some("rs") {
            return;
        }
        let relative_path = file_path.strip_prefix(absolute_workspace_root).unwrap_or(file_path).to_path_buf();
        let node_name = relative_path.to_string_lossy().into_owned();
        let bytes = std::fs::read(file_path).ok();
        ix.scan_file(&node_name, bytes.as_deref(), depth, extract_symbols);
    }

    // The lexer's entry points, kept on the scanner for its callers and tests; the bodies are the core's.
    #[cfg_attr(not(test), allow(dead_code))]
    fn strip_comments(content: &str) -> String {
        matrix_core::index::strip_comments(content)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn split_statements(content: &str) -> Vec<String> {
        matrix_core::index::split_statements(content)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn extract_deps_from_stmt(stmt: &str) -> Vec<String> {
        matrix_core::index::extract_deps_from_stmt(stmt)
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
    let mut facet_receipts: u64 = 0;
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
                    // SR29: an opened image is Facet's — delegate, after the Finder's own answer.
                    let delegate = finder::facet_delegation(finder.root(), &out, finder::FACET_RECEIPT_TAG | facet_receipts);
                    synapse.fire_async(SMessage::Matrix(out)).await;
                    if let Some(open) = delegate {
                        facet_receipts += 1;
                        synapse.fire_async(SMessage::Facet(open)).await;
                    }
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
