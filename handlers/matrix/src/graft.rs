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

//! GraftTopology decode/apply — matrix-owned logic for surgically grafting a
//! single-file symbol scan onto an existing `TopologyNode` tree.
//!
//! Matrix emits `MatrixEvent::GraftTopology { target_id, payload }` where the
//! payload is the `DICTIONARY$EDGES` format produced by
//! `MatrixScanner::map_topology`. Vessels (lumen) should stay wiring-only:
//! they call [`apply_graft`] on their tree and re-render on `true`.

use bandy::state::TopologyNode;

/// Decode a `DICTIONARY$EDGES` payload into the symbol nodes belonging to
/// `target_id`. Each symbol becomes a leaf `TopologyNode` with id
/// `"{target_id}#{symbol}"`. Returns an empty vec when the payload is
/// malformed (no `$` separator) or holds no edge for `target_id`.
pub fn decode_graft_symbols(payload: &str, target_id: &str) -> Vec<TopologyNode> {
    let mut symbols_to_graft = Vec::new();

    let Some((dict_str, edges_str)) = payload.split_once('$') else {
        return symbols_to_graft;
    };
    let dict_list: Vec<&str> = dict_str.split(',').collect();

    // Parse edges "NodeID:DepID,DepID" separated by '|'.
    for edge in edges_str.split('|') {
        if let Some((node_id_str, deps_str)) = edge.split_once(':') {
            if let Ok(node_id) = node_id_str.parse::<usize>() {
                if let Some(node_name) = dict_list.get(node_id) {
                    // Only the edge belonging to our target contributes symbols.
                    if *node_name == target_id {
                        for dep_id_str in deps_str.split(',') {
                            if let Ok(dep_id) = dep_id_str.parse::<usize>() {
                                if let Some(symbol_name) = dict_list.get(dep_id) {
                                    symbols_to_graft.push(TopologyNode {
                                        id: format!("{}#{}", target_id, symbol_name),
                                        label: symbol_name.to_string(),
                                        children: Vec::new(),
                                        is_expanded: false,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    symbols_to_graft
}

/// Find `target_id` in the forest (depth-first) and REPLACE its children with
/// `new_children`. Returns `true` if the target was found.
pub fn graft_into_tree(
    roots: &mut [TopologyNode],
    target_id: &str,
    new_children: Vec<TopologyNode>,
) -> bool {
    fn graft_to_node(
        node: &mut TopologyNode,
        target_id: &str,
        new_children: Vec<TopologyNode>,
    ) -> bool {
        if node.id == target_id {
            node.children = new_children;
            return true;
        }
        for child in &mut node.children {
            if graft_to_node(child, target_id, new_children.clone()) {
                return true;
            }
        }
        false
    }

    for root in roots.iter_mut() {
        if graft_to_node(root, target_id, new_children.clone()) {
            return true;
        }
    }
    false
}

/// Decode `payload` and graft the resulting symbols onto `target_id` in the
/// forest. Returns `true` when the tree changed (target found in a
/// well-formed payload) — the caller should re-render on `true`.
pub fn apply_graft(roots: &mut [TopologyNode], target_id: &str, payload: &str) -> bool {
    if !payload.contains('$') {
        return false;
    }
    let symbols = decode_graft_symbols(payload, target_id);
    graft_into_tree(roots, target_id, symbols)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, children: Vec<TopologyNode>) -> TopologyNode {
        TopologyNode {
            id: id.to_string(),
            label: id.rsplit('/').next().unwrap_or(id).to_string(),
            children,
            is_expanded: false,
        }
    }

    /// dict: 0=src/main.rs, 1="fn main", 2="struct App"; edge for node 0.
    const PAYLOAD: &str = "src/main.rs,fn main,struct App$0:1,2";

    #[test]
    fn decode_extracts_target_symbols() {
        let symbols = decode_graft_symbols(PAYLOAD, "src/main.rs");
        let got: Vec<(&str, &str)> = symbols
            .iter()
            .map(|n| (n.id.as_str(), n.label.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("src/main.rs#fn main", "fn main"),
                ("src/main.rs#struct App", "struct App"),
            ]
        );
        assert!(symbols.iter().all(|n| n.children.is_empty() && !n.is_expanded));
    }

    #[test]
    fn decode_ignores_other_targets_and_malformed_payloads() {
        assert!(decode_graft_symbols(PAYLOAD, "src/other.rs").is_empty());
        assert!(decode_graft_symbols("no-dollar-separator", "src/main.rs").is_empty());
        assert!(decode_graft_symbols("a,b$bogus:edge|9:1", "a").is_empty());
    }

    #[test]
    fn graft_replaces_children_of_nested_target() {
        let mut roots = vec![node(
            "src",
            vec![node("src/main.rs", vec![node("stale#old", vec![])])],
        )];
        let grafted = graft_into_tree(
            &mut roots,
            "src/main.rs",
            vec![node("src/main.rs#fn main", vec![])],
        );
        assert!(grafted);
        let target = &roots[0].children[0];
        assert_eq!(target.children.len(), 1);
        assert_eq!(target.children[0].id, "src/main.rs#fn main");
    }

    #[test]
    fn graft_returns_false_when_target_missing() {
        let mut roots = vec![node("src", vec![node("src/lib.rs", vec![])])];
        assert!(!graft_into_tree(&mut roots, "src/main.rs", vec![]));
        // Tree untouched.
        assert!(roots[0].children[0].children.is_empty());
    }

    #[test]
    fn apply_graft_end_to_end() {
        let mut roots = vec![node("src", vec![node("src/main.rs", vec![])])];
        assert!(apply_graft(&mut roots, "src/main.rs", PAYLOAD));
        assert_eq!(roots[0].children[0].children.len(), 2);

        // Malformed payload: no mutation, no re-render.
        assert!(!apply_graft(&mut roots, "src/main.rs", "garbage"));
        assert_eq!(roots[0].children[0].children.len(), 2);
    }
}
