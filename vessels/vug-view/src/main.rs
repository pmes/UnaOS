// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `vug-view` — Vug's first face (VUGFACE1, LEDGER SR470): a window over a mesh.
//!
//! Like every vessel this binary is wiring and lifecycle only. It opens the file through the model seam (mesh_core's
//! readers), and runs the [`vug_view::Viewer`] controller between the window and the rasteriser: window input moves the
//! view, each settled burst of input becomes one `SurfaceBlit`. On macOS the window is quartzite's image surface
//! (`bootstrap_image_surface`); everywhere, the headless witness drives the same controller from a key script and
//! writes the last frame as a PNG:
//!
//! ```text
//! vug-view <file.stl|obj|ply> [--preset una|solidworks|fusion|blender|onshape]    # the window (macOS)
//! vug-view <file> --size 320x240 --keys "17h" --out shot.png                      # headless witness
//! ```
//! Key script: one character per shortcut; `{Name}` for a named key.

use bandy::SMessage;
use vug_view::view::Preset;
use vug_view::Viewer;

struct Cli {
    path: String,
    size: (u32, u32),
    preset: Preset,
    keys: Option<String>,
    out: Option<String>,
}

const USAGE: &str = "usage: vug-view <file.stl|obj|ply|png|jpg> [--preset una|solidworks|fusion|blender|onshape] [--size WxH] [--keys SCRIPT] [--out shot.png]";

fn cli() -> Cli {
    let mut a = std::env::args().skip(1);
    let mut c = Cli { path: String::new(), size: (800, 600), preset: Preset::Una, keys: None, out: None };
    while let Some(arg) = a.next() {
        match arg.as_str() {
            "--size" => {
                let v = a.next().unwrap_or_default();
                let (w, h) = v.split_once('x').unwrap_or(("", ""));
                c.size = (w.parse().unwrap_or(800), h.parse().unwrap_or(600));
            }
            "--preset" => {
                let v = a.next().unwrap_or_default();
                c.preset = Preset::parse(&v).unwrap_or_else(|| {
                    eprintln!("vug-view: unknown preset {v:?}; {USAGE}");
                    std::process::exit(2);
                });
            }
            "--keys" => c.keys = a.next(),
            "--out" => c.out = a.next(),
            "--help" | "-h" => {
                eprintln!("{USAGE}");
                for (k, what) in vug_view::SHORTCUTS {
                    eprintln!("  {k:<10} {what}");
                }
                std::process::exit(0);
            }
            _ => c.path = arg,
        }
    }
    if c.path.is_empty() {
        eprintln!("vug-view: {USAGE}");
        std::process::exit(2);
    }
    c
}

/// Split a key script into keys: single characters, or `{Name}` tokens.
fn keys(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = script.chars();
    while let Some(c) = it.next() {
        if c == '{' {
            out.push(it.by_ref().take_while(|&c| c != '}').collect());
        } else {
            out.push(c.to_string());
        }
    }
    out
}

fn main() {
    let cli = cli();
    // VUGFACEPHOTO (SR506): the one open road — a mesh, or a photo on graph paper measured into a sketch.
    let mut viewer = match Viewer::open(&cli.path, cli.size, cli.preset) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("vug-view: {e}");
            std::process::exit(1);
        }
    };
    #[allow(unused_variables)] // the window's title (macOS only); the headless witness prints `viewer.title`
    let title = viewer.title.clone();

    // ---- The headless witness: the same controller, frames to a PNG. ----
    if cli.out.is_some() || cfg!(not(target_os = "macos")) {
        let Some(out) = cli.out.clone() else {
            eprintln!("vug-view: no native window backend on this platform yet (macOS first); use --out");
            std::process::exit(2);
        };
        for k in keys(cli.keys.as_deref().unwrap_or("")) {
            viewer.key(&k);
        }
        let SMessage::SurfaceBlit { width, height, pixels, .. } = viewer.frame() else { unreachable!("frame() is a SurfaceBlit") };
        let png = match pixel_core::png::encode::encode_rgba(width, height, &pixels) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("vug-view: png: {e:?}");
                std::process::exit(1);
            }
        };
        if let Err(e) = std::fs::write(&out, png) {
            eprintln!("vug-view: {out}: {e}");
            std::process::exit(1);
        }
        println!("{} -> {out}", viewer.title);
        std::process::exit(0);
    }

    // ---- The window (macOS AppKit via quartzite's image surface). Read-checked only: no Mac on the build seat. ----
    #[cfg(target_os = "macos")]
    {
        use gneiss_pal::paths::UnaPaths;
        use tokio::sync::broadcast::error::{RecvError, TryRecvError};
        UnaPaths::awaken().expect("CRITICAL: Failed to awaken spatial paths");
        bandy::telemetry::ignite(UnaPaths::root().join("logs"));
        log::info!("[VUG-VIEW] :: boot — {} ({} triangles, preset {})", cli.path, viewer.model.triangle_count(), viewer.view.preset.name());
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
        let synapse = bandy::Synapse::new();

        // The controller loop (never the AppKit main thread, which only blits): take one message, drain every queued
        // one, render ONE frame for the burst.
        {
            let synapse = synapse.clone();
            let mut rx = synapse.subscribe();
            rt.spawn(async move {
                // The surface is built after this task starts (new_vessel, main thread): the first frame waits for it
                // to subscribe (its first BrowserResize also asks for one).
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                synapse.fire(viewer.frame());
                loop {
                    let mut dirty = match rx.recv().await {
                        Ok(m) => viewer.input(&m),
                        Err(RecvError::Lagged(_)) => true,
                        Err(RecvError::Closed) => break,
                    };
                    loop {
                        match rx.try_recv() {
                            Ok(m) => dirty |= viewer.input(&m),
                            Err(TryRecvError::Lagged(_)) => dirty = true,
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Closed) => return,
                        }
                    }
                    for m in viewer.outbox() { synapse.fire(m); } // VUGFACEINTENTSEND (SR514): the face's asks onto the bus
                    if dirty {
                        synapse.fire(viewer.frame());
                    }
                }
            });
        }

        let surface_synapse = synapse.clone();
        quartzite::Backend::new_vessel("org.unaos.vug", &format!("vug — {title}"), (cli.size.0 as f64, cli.size.1 as f64), move |_window| {
            quartzite::platforms::macos::image_view::bootstrap_image_surface(vug_view::SURFACE, surface_synapse.clone())
        })
        .run();
        drop(rt);
    }
}
