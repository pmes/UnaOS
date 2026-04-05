// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

mod core;
mod cortex;

#[allow(unused_imports)]
use bandy::{SMessage, Synapse, telemetry};
use gneiss_pal::AppHandler;
#[allow(unused_imports)]
use std::sync::{Arc, RwLock};
use gneiss_pal::paths::UnaPaths;
use quartzite::{self, Backend, NativeWindow};
use std::rc::Rc;
use vein::VeinHandler;

fn main() {
    // 0. Ignite the Substrate Reactor (Tokio)
    let rt = tokio::runtime::Runtime::new().expect("CRITICAL: Failed to ignite Tokio reactor");
    let _guard = rt.enter();

    let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);

    // Spawn Signal Interceptor Task
    let signal_tx = shutdown_tx.clone();
    rt.spawn(async move {
        let mut sigint =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).unwrap();
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        tokio::select! {
            _ = sigint.recv() => {
                log::info!("\n[UNAOS] :: SIGINT Caught. Initiating Graceful Shutdown...\n");
                let _ = signal_tx.send(());
            }
            _ = sigterm.recv() => {
                log::info!("\n[UNAOS] :: SIGTERM Caught. Initiating Graceful Shutdown...\n");
                let _ = signal_tx.send(());
            }
        }

        // Use CXX-Qt's thread queue or FFI call to explicitly terminate the Qt application.
        // This unblocks the main thread.
        #[cfg(feature = "qt")]
        quartzite::platforms::qt::ffi::quit_qapplication();
    });

    // 1. Establish Base Camp
    UnaPaths::awaken().expect("CRITICAL: Failed to awaken spatial paths");

    // Split the brain
    let vein_storage = UnaPaths::primary_vault();
    let cortex_vault = UnaPaths::subconscious_vault();

    // 2. Ignite Telemetry
    telemetry::ignite(UnaPaths::root().join("logs"));
    log::info!("Lumen Boot Sequence Initiated.");

    // 3. Ignite the Spine
    let synapse = Synapse::new();

    // 4. Initialize Crypto
    let _ = rustls::crypto::ring::default_provider().install_default();

    // 5. Awaken the Autonomous Core
    let core_synapse = synapse.clone();
    let shutdown_rx_core = shutdown_tx.subscribe();
    let core_handle = rt.spawn(async move {
        core::ignite(cortex_vault, core_synapse, shutdown_rx_core).await;
    });

    // 5.5 Ignite Amber Bytes Storage Rune
    let amber_synapse = synapse.clone();
    let amber_vault_path = vein_storage.clone();
    let amber_handle = rt.spawn(async move {
        amber_bytes::ignite(amber_vault_path, amber_synapse).await;
    });

    // J21 PATHFINDER: Resolve absolute workspace root zero-latency anchor exactly once
    let absolute_workspace_root = elessar::find_workspace_root();
    log::info!("[ELESSAR] Absolute Workspace Root Anchored: {:?}", absolute_workspace_root);
    let absolute_workspace_root_arc = std::sync::Arc::new(absolute_workspace_root);

    // 5.7 Ignite Matrix Spatial Mapper
    let matrix_synapse = synapse.clone();
    let matrix_root_arc = absolute_workspace_root_arc.clone();
    let matrix_handle = rt.spawn(async move {
        matrix::ignite(matrix_synapse, matrix_root_arc).await;
    });

    // 6. Ignite the AI Handler (The Conscious Vein)
    let mut default_state = bandy::state::AppState::default();
    default_state.absolute_workspace_root = absolute_workspace_root_arc.clone();

    let app_state = Arc::new(RwLock::new(default_state));
    // Channels for UI Events (Spline -> Vein)
    let (event_tx, event_rx) = async_channel::unbounded::<bandy::SMessage>();

    // 7.5. Define the Workspace Layout via Declarative UI Engine
    let genesis_roots = matrix::MatrixScanner::build_genesis_tree(&absolute_workspace_root_arc, &absolute_workspace_root_arc);
    let workspace_state = bandy::state::WorkspaceState {
        left_pane: bandy::state::ViewEntity::Topology(bandy::state::TopologyState::new(genesis_roots)),
        right_pane: bandy::state::ViewEntity::Stream(bandy::state::StreamState::default()),
        split_ratio: 0.25,
    };

    let workspace_state_clone = workspace_state.clone();

    // We move VeinHandler into a separate task.
    // Since VeinHandler is "Pure Logic", it should run on Tokio.
    // The `handle_event` method processes events from the UI.

    let (shutdown_tx_vein, shutdown_rx_vein) = (shutdown_tx.clone(), shutdown_tx.subscribe());
    let (vein_handler, bg_handle) = VeinHandler::new(
        vein_storage,
        synapse.clone(),
        app_state.clone(),
        shutdown_tx_vein,
    );
    let synapse_event_loop = synapse.clone();

    // Spawn the Brain Loop
    let brain_loop_handle = rt.spawn(async move {
        let mut vein = vein_handler;
        let mut shutdown_rx = shutdown_rx_vein;
        let mut workspace_state = workspace_state_clone;
        let mut synapse_rx = synapse_event_loop.subscribe();

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    log::info!(":: VEIN :: Brain Event Loop terminating cleanly.");
                    break;
                }
                event_res = event_rx.recv() => {
                    if let Ok(event) = event_res {
                        match event {
                            bandy::SMessage::UiReady => {
                                if let bandy::state::ViewEntity::Topology(ref mut matrix) = workspace_state.left_pane {
                                    let flat_tree = matrix.tree.flatten();
                                    let mapped_tree: Vec<(String, String, usize)> = flat_tree.into_iter().map(|(n, depth)| {
                                        (n.id.clone(), n.label.clone(), depth)
                                    }).collect();
                                    synapse_event_loop.fire(bandy::SMessage::Matrix(bandy::MatrixEvent::TopologyMutated(mapped_tree)));
                                }
                            }
                            bandy::SMessage::ToggleMatrixNode(id) => {
                                if let bandy::state::ViewEntity::Topology(ref mut matrix) = workspace_state.left_pane {
                                    matrix.tree.toggle_node(&id);
                                    let flat_tree = matrix.tree.flatten();

                                    let mapped_tree: Vec<(String, String, usize)> = flat_tree.into_iter().map(|(n, depth)| {
                                        (n.id.clone(), n.label.clone(), depth)
                                    }).collect();
                                    synapse_event_loop.fire(bandy::SMessage::Matrix(bandy::MatrixEvent::TopologyMutated(mapped_tree)));

                                    // Only fire the AST Matrix scan if the ID looks like a file (has an extension).
                                    // This prevents directories from vomiting their contents into the UI.
                                    let is_file = std::fs::metadata(&id).map(|m| m.is_file()).unwrap_or(false);
                                    if is_file {
                                        synapse_event_loop.fire(bandy::SMessage::Matrix(bandy::MatrixEvent::FocusSector(id)));
                                    }
                                }
                            }
                            _ => {
                                vein.handle_event(event);
                            }
                        }
                    } else {
                        break;
                    }
                }
                // --- MATRIX EVENTS ---
                // We handle Matrix events locally if they dictate UI structure mutations.
                msg = synapse_rx.recv() => {
                    if let Ok(bandy::SMessage::Matrix(bandy::MatrixEvent::GraftTopology { target_id, payload })) = msg {
                        if let bandy::state::ViewEntity::Topology(ref mut matrix) = workspace_state.left_pane {
                            // 1. Decompress payload: DICTIONARY$TOPOLOGY
                            if let Some((dict_str, edges_str)) = payload.split_once('$') {
                                let dict_list: Vec<&str> = dict_str.split(',').collect();

                                // Parse edges "NodeID:DepID,DepID"
                                let edges: Vec<&str> = edges_str.split('|').collect();

                                // Since it's a single file scan, the first edge should be the file's edge
                                // containing the symbol IDs.
                                let mut symbols_to_graft = Vec::new();
                                for edge in edges {
                                    if let Some((node_id_str, deps_str)) = edge.split_once(':') {
                                        if let Ok(node_id) = node_id_str.parse::<usize>() {
                                            if let Some(node_name) = dict_list.get(node_id) {
                                                // Check if this edge belongs to our target_id
                                                if *node_name == target_id {
                                                    for dep_id_str in deps_str.split(',') {
                                                        if let Ok(dep_id) = dep_id_str.parse::<usize>() {
                                                            if let Some(symbol_name) = dict_list.get(dep_id) {
                                                                // Valid symbol! Instantiate a TopologyNode
                                                                symbols_to_graft.push(bandy::state::TopologyNode {
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

                                // 2. Traverse matrix.tree to find `target_id` and append the nodes
                                fn graft_to_node(node: &mut bandy::state::TopologyNode, target_id: &str, new_children: Vec<bandy::state::TopologyNode>) -> bool {
                                    if node.id == target_id {
                                        // Found the target node, replace its children (or append)
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

                                let mut grafted = false;
                                for root_node in &mut matrix.tree.roots {
                                    if graft_to_node(root_node, &target_id, symbols_to_graft.clone()) {
                                        grafted = true;
                                        break;
                                    }
                                }

                                if grafted {
                                    // 3. Re-render: flatten tree and broadcast Mutation
                                    let flat_tree = matrix.tree.flatten();
                                    let mapped_tree: Vec<(String, String, usize)> = flat_tree.into_iter().map(|(n, depth)| {
                                        (n.id.clone(), n.label.clone(), depth)
                                    }).collect();
                                    synapse_event_loop.fire(bandy::SMessage::Matrix(bandy::MatrixEvent::TopologyMutated(mapped_tree)));
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    // 7. View & Engine Ignition
    let spline = Rc::new(quartzite::Spline::new());

    #[cfg(feature = "qt")]
    {
        // J24.5 ROUBAIX: Synchronously inject the state before the UI boot to ensure MatrixModel Rust has data on first frame.
        let _ = quartzite::platforms::qt::vein_bridge::WORKSPACE_STATE.set(workspace_state.clone());
    }

    // THE FUSION
    #[cfg(target_os = "macos")]
    let bootstrap = move |
        window: &NativeWindow,
        tx_event: async_channel::Sender<bandy::SMessage>,
        app_state_ref: std::sync::Arc<std::sync::RwLock<bandy::state::AppState>>,
        rx_synapse: tokio::sync::broadcast::Receiver<bandy::SMessage>,
        workspace_state_ref: bandy::state::WorkspaceState,
    | -> quartzite::BootstrapPayload {
        let vein_widget = spline.bootstrap(
            window,
            tx_event,
            app_state_ref,
            rx_synapse,
            &workspace_state_ref,
        );
        vein_widget
    };

    #[cfg(not(target_os = "macos"))]
    let bootstrap = move |window: &NativeWindow| -> quartzite::BootstrapPayload {
        let vein_widget = spline.bootstrap(
            window,
            event_tx.clone(),
            app_state.clone(),
            synapse.subscribe(),
            &workspace_state,
        );
        vein_widget
    };

    #[cfg(target_os = "macos")]
    Backend::new("org.unaos.lumen", event_tx.clone(), app_state.clone(), synapse.subscribe(), workspace_state.clone(), bootstrap).run();

    #[cfg(not(target_os = "macos"))]
    Backend::new("org.unaos.lumen", bootstrap).run();

    // Broadcast shutdown in case GUI exited naturally instead of via SIGINT/SIGTERM
    let _ = shutdown_tx.send(());

    // 1. Wait for UI tasks to sync and finish
    rt.block_on(async {
        let _ = brain_loop_handle.await;
        let _ = bg_handle.await;
        let _ = core_handle.await;
        matrix_handle.abort();
        amber_handle.abort();
    });
}
