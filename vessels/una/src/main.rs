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

//! UnaIDE ("una") — the IDE vessel, revived on the current quartzite/bandy API.
//!
//! This is the "lumen-lite" wiring: the same declarative `WorkspaceState` +
//! `Synapse` broadcast bus + macOS `Backend`/`Spline` seam that lumen uses, but
//! with no Vein/AI cortex. The brain loop serves only the two structural
//! signals a bare workspace needs — `UiReady` (first-frame topology render) and
//! `ToggleMatrixNode` (expand/collapse + focus). Left pane is the Matrix
//! topology of the cwd; right pane is a Stream. Tabula (the editor view) is
//! GTK-locked on macOS and returns in a later arc.

#[allow(unused_imports)]
use bandy::{SMessage, Synapse};
use quartzite::{self, Backend, NativeWindow};
use std::rc::Rc;
use std::sync::{Arc, RwLock};

const APP_ID: &str = "org.unaos.UnaIDE";

fn main() {
    println!(":: UNA :: WAKING UP THE FORGE...");

    // 0. Ignite the Substrate Reactor (Tokio)
    let rt = tokio::runtime::Runtime::new().expect("CRITICAL: Failed to ignite Tokio reactor");
    let _guard = rt.enter();

    let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);

    // Spawn Signal Interceptor Task (graceful shutdown on SIGINT/SIGTERM)
    let signal_tx = shutdown_tx.clone();
    rt.spawn(async move {
        let mut sigint =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).unwrap();
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        tokio::select! {
            _ = sigint.recv() => {
                println!("\n[UNA] :: SIGINT Caught. Initiating Graceful Shutdown...\n");
                let _ = signal_tx.send(());
            }
            _ = sigterm.recv() => {
                println!("\n[UNA] :: SIGTERM Caught. Initiating Graceful Shutdown...\n");
                let _ = signal_tx.send(());
            }
        }
    });

    // 1. Ignite the Spine (broadcast synapse) + crypto provider
    let synapse = Synapse::new();
    let _ = rustls::crypto::ring::default_provider().install_default();

    // 2. Anchor the workspace root at the cwd.
    let absolute_workspace_root = std::env::current_dir().unwrap_or_default();
    println!("[UNA] Workspace Root Anchored: {:?}", absolute_workspace_root);
    let absolute_workspace_root_arc = Arc::new(absolute_workspace_root);

    // 3. Ignite Matrix Spatial Mapper (consumes FocusSector, emits GraftTopology).
    let matrix_synapse = synapse.clone();
    let matrix_root_arc = absolute_workspace_root_arc.clone();
    let matrix_handle = rt.spawn(async move {
        matrix::ignite(matrix_synapse, matrix_root_arc).await;
    });

    // 4. Shared app state (no cortex — just the anchored root).
    let mut default_state = bandy::state::AppState::default();
    default_state.absolute_workspace_root = absolute_workspace_root_arc.clone();
    let app_state = Arc::new(RwLock::new(default_state));

    // Channels for UI events (Spline -> brain loop).
    let (event_tx, event_rx) = async_channel::unbounded::<bandy::SMessage>();

    // 5. Declarative workspace layout: Topology (left) + Stream (right).
    let genesis_roots = matrix::MatrixScanner::build_genesis_tree(
        &absolute_workspace_root_arc,
        &absolute_workspace_root_arc,
    );
    let workspace_state = bandy::state::WorkspaceState {
        left_pane: bandy::state::ViewEntity::Topology(bandy::state::TopologyState::new(genesis_roots)),
        right_pane: bandy::state::ViewEntity::Stream(bandy::state::StreamState::default()),
        split_ratio: 0.25,
    };
    let workspace_state_clone = workspace_state.clone();

    // 6. The Brain Loop — minimal (no Vein). Serves structural signals only:
    //    UiReady (first render) and ToggleMatrixNode (expand/collapse + focus).
    let synapse_event_loop = synapse.clone();
    let shutdown_rx_brain = shutdown_tx.subscribe();
    let brain_loop_handle = rt.spawn(async move {
        let mut shutdown_rx = shutdown_rx_brain;
        let mut workspace_state = workspace_state_clone;
        let mut synapse_rx = synapse_event_loop.subscribe();

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    println!(":: UNA :: Brain Event Loop terminating cleanly.");
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

                                    // Only fire the AST Matrix scan if the ID looks like a file.
                                    let is_file = std::fs::metadata(&id).map(|m| m.is_file()).unwrap_or(false);
                                    if is_file {
                                        synapse_event_loop.fire(bandy::SMessage::Matrix(bandy::MatrixEvent::FocusSector(id)));
                                    }
                                }
                            }
                            // No Vein: every other UI impulse is a no-op for now.
                            _ => {}
                        }
                    } else {
                        break;
                    }
                }
                // Matrix events that mutate UI structure are grafted locally.
                msg = synapse_rx.recv() => {
                    if let Ok(bandy::SMessage::Matrix(bandy::MatrixEvent::GraftTopology { target_id, payload })) = msg {
                        if let bandy::state::ViewEntity::Topology(ref mut matrix) = workspace_state.left_pane {
                            if matrix::graft::apply_graft(&mut matrix.tree.roots, &target_id, &payload) {
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
    });

    // 7. View & Engine Ignition
    let spline = Rc::new(quartzite::Spline::new());

    // THE FUSION — the macOS Backend drives the bootstrap closure on the main
    // thread once AppKit has launched, threading its own copies of the bus ends.
    #[cfg(target_os = "macos")]
    let bootstrap = move |
        window: &NativeWindow,
        tx_event: async_channel::Sender<bandy::SMessage>,
        app_state_ref: std::sync::Arc<std::sync::RwLock<bandy::state::AppState>>,
        rx_synapse: tokio::sync::broadcast::Receiver<bandy::SMessage>,
        workspace_state_ref: bandy::state::WorkspaceState,
    | -> quartzite::BootstrapPayload {
        spline.bootstrap(
            window,
            tx_event,
            app_state_ref,
            rx_synapse,
            &workspace_state_ref,
        )
    };

    #[cfg(not(target_os = "macos"))]
    let bootstrap = move |window: &NativeWindow| -> quartzite::BootstrapPayload {
        spline.bootstrap(
            window,
            event_tx.clone(),
            app_state.clone(),
            synapse.subscribe(),
            &workspace_state,
        )
    };

    #[cfg(target_os = "macos")]
    Backend::new(APP_ID, event_tx.clone(), app_state.clone(), synapse.subscribe(), workspace_state.clone(), bootstrap).run();

    #[cfg(not(target_os = "macos"))]
    Backend::new(APP_ID, bootstrap).run();

    // Broadcast shutdown in case the GUI exited naturally instead of via a signal.
    let _ = shutdown_tx.send(());

    // Wait for the brain loop to drain; stop the matrix mapper.
    rt.block_on(async {
        let _ = brain_loop_handle.await;
        matrix_handle.abort();
    });
}
