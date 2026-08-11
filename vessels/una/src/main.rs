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
//! with no Vein/AI cortex. The brain loop serves the structural signals a bare
//! workspace needs — `UiReady` (first-frame topology render) and
//! `ToggleMatrixNode` (expand/collapse + focus) — plus live editor loading.
//!
//! The workspace shape is no longer hand-rolled: `elessar::Context` reads the
//! cwd, `.layout()` maps it to a `Layout`, and `elessar::workspace_for` builds
//! the `WorkspaceState`. In this repo the cwd resolves to a recognized project
//! (`Layout::Code`) → left pane Matrix topology, right pane **Editor**. An
//! unrecognized directory (the Void → `Layout::Comms`) falls back to a Stream
//! on the right. When a sidebar *file* is activated, the brain loop loads it
//! through `tabula::TabulaDocument` and broadcasts `SMessage::EditorLoad` so
//! quartzite's editor pane renders it.
//!
//! A path may also be named on the command line. There are **no option flags**:
//! opening a log is not a mode you type. A console/serial log (`UNAOS.LOG`, a
//! `ttyUSB*.log` capture, a squawk `*.out`) is *name-routed* to Tabula's
//! read-only Console view by `TabulaDocument::open`, so `una UNAOS.LOG` opens it
//! read-only on its own. That case overrides the Layout's right pane, because a
//! log's directory is not a `Layout::Code` project.
//!
//! The read-only treatment is **not an app policy**: on the shard a kernel
//! flight-recorder log is owned by *root*, and the UnaFS ownership ACL
//! (`acl-<lba>-<off>` rows carrying `owner`/`grants:*` — see `docs/SECURITY.md`)
//! denies a user-owned vessel any write, full stop. The view only renders the
//! record; `TabulaDocument::read_only` mirrors that filesystem fact for the host
//! path, it does not invent it.
//!
//! Summoning the *newest* log without naming it is the **Console app's** job —
//! a desktop tile backed by `comscan` (the future `ViewEntity::Console`), not a
//! command-line flag. `tabula::default_console_log` is the discovery seam that
//! tile reuses; `una` no longer wires it to any flag.

#[allow(unused_imports)]
use bandy::{SMessage, Synapse};
use quartzite::{self, Backend, NativeWindow};
use std::rc::Rc;
use std::sync::{Arc, RwLock};

const APP_ID: &str = "org.unaos.UnaIDE";

/// Resolve the command line into `(workspace_root, file_to_open)`.
///
/// `una` takes at most one **positional** argument and **no option flags** —
/// opening a log is a file you point the app at, not a mode you type. The
/// grammar (GUI-free, so it is unit-testable):
///
///     una           -> (None, None)               workspace at the cwd
///     una <dir>     -> (Some(dir), None)           workspace at <dir>
///     una <file>    -> (Some(parent), Some(file))  open <file>, anchored beside it
///
/// How a named file opens is decided downstream by `TabulaDocument::open`:
/// a console/serial log (`UNAOS.LOG`, `ttyUSB*.log`, squawk `*.out`) is
/// name-routed to the read-only Console view; everything else opens editable.
/// There is deliberately no flag to force either treatment — the log's
/// read-only-ness comes from the file's ownership (root, on the shard), not
/// from a command-line mode, and the "newest log with no name" summon belongs
/// to the Console app tile, not to `una`'s argv.
fn parse_args<I: IntoIterator<Item = String>>(
    args: I,
) -> Result<(Option<std::path::PathBuf>, Option<std::path::PathBuf>), String> {
    let mut positional: Option<String> = None;

    for arg in args {
        match arg.as_str() {
            a if a.starts_with('-') => {
                return Err(format!(
                    "unknown option {:?} — `una` takes a PATH, not flags",
                    a
                ))
            }
            a => {
                if positional.is_some() {
                    return Err("at most one PATH may be given".to_string());
                }
                positional = Some(a.to_string());
            }
        }
    }

    match positional.map(std::path::PathBuf::from) {
        None => Ok((None, None)),
        // A directory argument is a workspace root, not a file to open.
        Some(p) if p.is_dir() => Ok((Some(p), None)),
        Some(p) => {
            if !p.is_file() {
                return Err(format!("{}: not a readable file", p.display()));
            }
            // Anchor the workspace beside the file so the sidebar shows its
            // neighbours.
            let root = p
                .parent()
                .filter(|q| !q.as_os_str().is_empty())
                .map(|q| q.to_path_buf());
            Ok((root, Some(p)))
        }
    }
}

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

    // 1b. Command line. `una` takes at most one positional argument and no
    //     option flags:
    //
    //       una           — workspace at the cwd
    //       una <dir>     — workspace at <dir>
    //       una <file>    — workspace at the file's parent, file open
    //
    //     A named log opens read-only on its own: `TabulaDocument::open`
    //     name-routes it to the Console view. A console log is not a workspace
    //     member — it lives on the shard's FAT volume once the card is mounted,
    //     or in a capture directory — so the flow is "open this path", not
    //     "browse to it in the sidebar". Summoning the *newest* log with no
    //     path is the Console app tile's job (comscan / `ViewEntity::Console`),
    //     not a flag on `una`.
    let (root_arg, open_arg) = match parse_args(std::env::args().skip(1)) {
        Ok(pair) => pair,
        Err(msg) => {
            eprintln!("[UNA] {}", msg);
            eprintln!("usage: una [PATH]");
            std::process::exit(2);
        }
    };

    // 2. Anchor the workspace root at the cwd (or the argument).
    let absolute_workspace_root = root_arg
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    println!("[UNA] Workspace Root Anchored: {:?}", absolute_workspace_root);
    let absolute_workspace_root_arc = Arc::new(absolute_workspace_root);

    // 3. Ignite Matrix Spatial Mapper (consumes FocusSector, emits GraftTopology).
    let matrix_synapse = synapse.clone();
    let matrix_root_arc = absolute_workspace_root_arc.clone();
    let matrix_handle = rt.spawn(async move {
        matrix::ignite(matrix_synapse, matrix_root_arc).await;
    });

    // 4. Shared app state (no cortex — just the anchored root).
    let default_state = bandy::state::AppState {
        absolute_workspace_root: absolute_workspace_root_arc.clone(),
        ..Default::default()
    };
    let app_state = Arc::new(RwLock::new(default_state));

    // Channels for UI events (Spline -> brain loop).
    let (event_tx, event_rx) = async_channel::unbounded::<bandy::SMessage>();

    // 5. Declarative workspace layout, resolved through elessar's Context.
    //    The genesis tree the matrix mapper scans becomes the left Topology;
    //    the cwd's Layout decides the right pane (Code → Editor, Comms → Stream).
    let genesis_roots = matrix::MatrixScanner::build_genesis_tree(
        &absolute_workspace_root_arc,
        &absolute_workspace_root_arc,
    );
    let context = elessar::Context::new(&absolute_workspace_root_arc);
    let layout = context.layout();
    println!("[UNA] Context Spline: {:?} → Layout: {:?}", context.spline, layout);
    let mut workspace_state = elessar::workspace_for(layout, genesis_roots);

    // 5b. A named file overrides the Layout's choice of right pane: a console
    //     log lives on a mounted FAT volume or in a capture directory, neither
    //     of which resolves to a `Layout::Code` project, so the Editor has to
    //     be asked for. Seeding `EditorState` here — rather than firing
    //     `EditorLoad` after `UiReady` — puts the text in the FIRST frame.
    let initial_document = open_arg.as_ref().and_then(|path| {
        // `open` name-routes: a log opens read-only in the Console view, any
        // other file opens editable. There is no flag override — a log is
        // read-only because it is not the viewer's to write, not because argv
        // said so.
        match tabula::TabulaDocument::open(path) {
            Ok(doc) => {
                println!(
                    "[UNA] Opened {} ({} byte(s){})",
                    path.display(),
                    doc.buffer.len(),
                    if doc.read_only { ", read-only console view" } else { "" }
                );
                Some(doc)
            }
            Err(e) => {
                eprintln!("[UNA] cannot open {}: {}", path.display(), e);
                None
            }
        }
    });
    if let Some(doc) = &initial_document {
        workspace_state.right_pane = bandy::state::ViewEntity::Editor(bandy::state::EditorState {
            path: doc.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            content: doc.buffer.clone(),
            language: doc.language.clone(),
        });
    }
    // A Code layout gets the console bottom pane (opt-in per elessar's seam):
    // an Empty ViewEntity requests the console; quartzite builds it, and the
    // brain loop feeds it via ConsoleAppend / drains it via ConsoleInput.
    if matches!(layout, elessar::Layout::Code) {
        workspace_state.bottom_pane = Some(bandy::state::ViewEntity::Empty);
    }
    let workspace_state_clone = workspace_state.clone();

    // 6. The Brain Loop — minimal (no Vein). Serves structural signals:
    //    UiReady (first render), ToggleMatrixNode (expand/collapse + focus),
    //    and — when the right pane is an Editor — live file loading.
    let synapse_event_loop = synapse.clone();
    let shutdown_rx_brain = shutdown_tx.subscribe();
    let brain_root = absolute_workspace_root_arc.clone();
    let brain_loop_handle = rt.spawn(async move {
        let mut shutdown_rx = shutdown_rx_brain;
        let mut workspace_state = workspace_state_clone;
        let mut synapse_rx = synapse_event_loop.subscribe();

        // Console back-ends, rooted at the workspace: `midden` runs shell
        // commands (its cwd mutates on `cd`), `aule` streams build output.
        let mut midden = midden::Midden::new_at(brain_root.as_ref().clone());
        let aule = aule::Aule::new(brain_root.as_path());

        // The live editor document. Empty until a file is activated. Only
        // meaningful when the right pane is an Editor (Layout::Code); harmless
        // otherwise.
        let editor_active =
            matches!(workspace_state.right_pane, bandy::state::ViewEntity::Editor(_));
        let mut document = initial_document.unwrap_or_else(tabula::TabulaDocument::new);

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
                                // Re-assert the held document on a live Editor
                                // pane so it starts in a known state: empty
                                // when nothing was opened, the console log (or
                                // named file) when it was.
                                if editor_active {
                                    synapse_event_loop.fire(bandy::SMessage::EditorLoad {
                                        path: document.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                                        content: document.buffer.clone(),
                                        language: document.language.clone(),
                                    });
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

                                    // A leaf whose id resolves to a real file is
                                    // a file *activation* (lumen's discrimination).
                                    let is_file = std::fs::metadata(&id).map(|m| m.is_file()).unwrap_or(false);
                                    if is_file {
                                        // (a) Ask the matrix mapper to graft this
                                        //     file's symbols into the topology.
                                        synapse_event_loop.fire(bandy::SMessage::Matrix(bandy::MatrixEvent::FocusSector(id.clone())));

                                        // (b) When an Editor pane is live, load the
                                        //     file through the portable Tabula core
                                        //     and broadcast it for quartzite to render.
                                        //     `open` routes console/serial logs
                                        //     (`UNAOS.LOG`, `*.log`, squawk `*.out`)
                                        //     into the read-only Console view.
                                        if editor_active {
                                            match tabula::TabulaDocument::open(&id) {
                                                Ok(doc) => {
                                                    document = doc;
                                                    synapse_event_loop.fire(bandy::SMessage::EditorLoad {
                                                        path: document.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                                                        content: document.buffer.clone(),
                                                        language: document.language.clone(),
                                                    });
                                                }
                                                Err(e) => {
                                                    eprintln!("[UNA] editor load failed for {:?}: {}", id, e);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            // Editor round-trip. The view fires EditorEdited on
                            // every text change; we mirror it into the held
                            // document (which marks it dirty).
                            bandy::SMessage::EditorEdited { content } => {
                                document.set_buffer(content);
                            }
                            // Cmd+S: persist the buffer and echo the outcome to
                            // the console so saves are visible. With no file
                            // loaded `save()` returns an error rather than
                            // panicking — that surfaces as a console line.
                            // A console log is a record: say WHY it is not
                            // written, without the io error's wording. This
                            // read-only view does not attempt the write — the
                            // save never leaves the app — and on the shard the
                            // log is root-owned, so the filesystem ACL would
                            // refuse it there too. State both without claiming
                            // an ACL acted on a write the view never issued.
                            bandy::SMessage::EditorSaveRequest if document.read_only => {
                                let name = document.path.as_ref()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_else(|| "<log>".to_string());
                                synapse_event_loop.fire(bandy::SMessage::ConsoleAppend(
                                    format!("[una] {} is a kernel log — read-only; this view won't write it, and on the shard it is root-owned so the filesystem ACL would refuse the write as well", name)));
                            }
                            bandy::SMessage::EditorSaveRequest => {
                                let line = match document.save() {
                                    Ok(()) => {
                                        let name = document.path.as_ref()
                                            .map(|p| p.display().to_string())
                                            .unwrap_or_else(|| "<untitled>".to_string());
                                        format!("[una] saved {}", name)
                                    }
                                    Err(e) => format!("[una] save failed: {}", e),
                                };
                                synapse_event_loop.fire(bandy::SMessage::ConsoleAppend(line));
                            }
                            // Console input. Built-ins first (`forge` → aule),
                            // everything else → midden (the held shell).
                            bandy::SMessage::ConsoleInput(raw) => {
                                let line = raw.trim();
                                if line.is_empty() {
                                    // nothing to run
                                } else if line == "forge" {
                                    let (ftx, frx) = std::sync::mpsc::channel::<String>();
                                    match aule.forge_streamed(ftx) {
                                        Ok(()) => {
                                            // aule's reader threads feed `frx`;
                                            // bridge each line onto the synapse
                                            // as a ConsoleAppend.
                                            let syn = synapse_event_loop.clone();
                                            std::thread::spawn(move || {
                                                for out in frx {
                                                    syn.fire(bandy::SMessage::ConsoleAppend(out));
                                                }
                                            });
                                        }
                                        Err(e) => {
                                            synapse_event_loop.fire(bandy::SMessage::ConsoleAppend(
                                                format!("[una] forge failed: {}", e)));
                                        }
                                    }
                                } else {
                                    let out = midden.execute(line);
                                    for l in out.stdout.lines() {
                                        synapse_event_loop.fire(bandy::SMessage::ConsoleAppend(l.to_string()));
                                    }
                                    for l in out.stderr.lines() {
                                        synapse_event_loop.fire(bandy::SMessage::ConsoleAppend(l.to_string()));
                                    }
                                    if out.exit_status != 0 {
                                        synapse_event_loop.fire(bandy::SMessage::ConsoleAppend(
                                            format!("[exit {}]", out.exit_status)));
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
                    if let Ok(bandy::SMessage::Matrix(bandy::MatrixEvent::GraftTopology { target_id, payload })) = msg
                        && let bandy::state::ViewEntity::Topology(ref mut matrix) = workspace_state.left_pane
                        && matrix::graft::apply_graft(&mut matrix.tree.roots, &target_id, &payload)
                    {
                        let flat_tree = matrix.tree.flatten();
                        let mapped_tree: Vec<(String, String, usize)> = flat_tree.into_iter().map(|(n, depth)| {
                            (n.id.clone(), n.label.clone(), depth)
                        }).collect();
                        synapse_event_loop.fire(bandy::SMessage::Matrix(bandy::MatrixEvent::TopologyMutated(mapped_tree)));
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

#[cfg(test)]
mod tests {
    use super::parse_args;
    use std::path::PathBuf;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Tests run in parallel in one process, so the scratch directory is
    /// named per test as well as per process — a shared name means one test
    /// deletes another's fixture mid-run.
    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("una_args_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn no_arguments_means_cwd_and_nothing_open() {
        assert_eq!(parse_args(args(&[])).unwrap(), (None, None));
    }

    #[test]
    fn a_directory_is_a_workspace_root() {
        let d = scratch("dir");
        let got = parse_args(args(&[d.to_str().unwrap()])).unwrap();
        assert_eq!(got, (Some(d.clone()), None));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A named log is a plain positional argument — no flag. It anchors the
    /// workspace beside itself; the read-only Console treatment is decided
    /// downstream by `TabulaDocument::open`, not by the argument grammar.
    #[test]
    fn a_log_file_opens_and_anchors_the_workspace_beside_it() {
        let d = scratch("file");
        let f = d.join("UNAOS.LOG");
        std::fs::write(&f, b":: log ::\n").unwrap();
        let (root, open) = parse_args(args(&[f.to_str().unwrap()])).unwrap();
        assert_eq!(root.as_deref(), Some(d.as_path()));
        assert_eq!(open.as_deref(), Some(f.as_path()));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The `--console`/`--edit` flag ceremony is GONE: both are rejected as
    /// unknown options, not accepted as modes.
    #[test]
    fn the_old_console_and_edit_flags_are_rejected() {
        let d = scratch("noflags");
        let f = d.join("notes.log");
        std::fs::write(&f, b"my own notes\n").unwrap();
        assert!(parse_args(args(&["--console"])).is_err());
        assert!(parse_args(args(&["--console", f.to_str().unwrap()])).is_err());
        assert!(parse_args(args(&["--edit", f.to_str().unwrap()])).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bad_arguments_are_rejected() {
        assert!(parse_args(args(&["--nope"])).is_err());
        assert!(parse_args(args(&["/a", "/b"])).is_err());
        assert!(parse_args(args(&["/definitely/not/here.log"])).is_err());
    }
}
