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
//! vug-view scan.ply --against part.vugdoc --out map.png                              # the deviation map (SR525)
//! vug-view flat.png --side side.png --keys "{Return}" --out part.png              # the side photo's thickness (SR547)
//! vug-view scan.ply --against part.vugdoc --gdt "<face>=flatness:0.5" --out map.png  # a gdt callout (SR553)
//! vug-view scan.ply --against part.vugdoc --datum A=<f> B=<f> C=<f> --gdt "<post>=position:1" --out map.png  # a datum frame (SR560)
//! vug-view part.vug --scan scan.ply --out map.png                                  # the part as the journal, a scan measured against it (SR565)
//! vug-view scan.ply --against part.vugdoc --gdt "<face>=flatness:0.5" --report     # the verdicts to part.vugreport beside the design (SR575)
//! vug-view flat.png --camera 400 --side square.png --out part.png                # the camera height to Principia's store, then the side (SR588; `--camera clear` / `--pp clear` unset)
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
    against: Option<String>, // VUGFACEDEVIATION (SR525): the design document a scan is measured against
    side: Option<String>, // VUGFACEPHOTOSIDE (SR547): a photo of the part on its side (the thickness proposal)
    gdt: Vec<String>, // VUGFACEGDT (SR553): each `--gdt <face>=<kind>:<tol>`, a callout on the map
    datum: Option<String>, // VUGFACEGDTREMEASURE (SR560): `--datum A=<f> B=<f> C=<f>`, the three args as one `datum` command
    scan: Option<String>, // VUGFACEDEVIATIONSOLIDFAULT (SR565): `--scan <mesh>` after a .vug part: the scan measured against the part's journal
    report: bool, // VUGFACEGDTREPORTKEY (SR575): `--report`, the map's `report` command after the open
    pref: Vec<String>, // VUGPHOTOPREFFACEWIRE (SR588): each `--camera` / `--pp` as one photo::command line, in order
}

const USAGE: &str = "usage: vug-view <file.stl|obj|ply|png|jpg> [--preset una|solidworks|fusion|blender|onshape] [--size WxH] [--keys SCRIPT] [--out shot.png] [--against design.vugdoc] [--side side.png] [--gdt face=kind:tol]... [--datum A=face B=face C=face] [--scan scan.ply (after a .vug part)] [--report] [--camera mm|clear] [--pp x y|clear]";

fn cli() -> Cli {
    let mut a = std::env::args().skip(1);
    let mut c = Cli { path: String::new(), size: (800, 600), preset: Preset::Una, keys: None, out: None, against: None, side: None, gdt: Vec::new(), datum: None, scan: None, report: false, pref: Vec::new() }; // VUGPHOTOPREFFACEWIRE (SR588): pref
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
            "--against" => c.against = a.next(), // VUGFACEDEVIATION (SR525)
            "--side" => c.side = a.next(), // VUGFACEPHOTOSIDE (SR547)
            "--gdt" => c.gdt.extend(a.next()), // VUGFACEGDT (SR553): repeatable
            "--scan" => c.scan = a.next(), // VUGFACEDEVIATIONSOLIDFAULT (SR565)
            "--report" => c.report = true, // VUGFACEGDTREPORTKEY (SR575)
            "--camera" => c.pref.push(format!("camera {}", a.next().unwrap_or_default())), // VUGPHOTOPREFFACEWIRE (SR588): the grammar is photo::command's
            "--pp" => c.pref.push(match a.next().unwrap_or_default() { v if v == "clear" => "pp clear".to_string(), x => format!("pp {x} {}", a.next().unwrap_or_default()) }), // VUGPHOTOPREFFACEWIRE (SR588): `x y` or `clear`
            "--datum" => c.datum = Some(a.by_ref().take(3).collect::<Vec<_>>().join(" ")), // VUGFACEGDTREMEASURE (SR560): the grammar is deviation::command's
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
    let prefs = vug_view::photo::PhotoPrefs::load(); // VUGFACEPHOTOCAMERAPREF (SR576): Principia's store, opened once at start (read-only)
    // VUGFACEPHOTO (SR506): the one open road — a mesh, or a photo on graph paper measured into a sketch.
    let opened = match (&cli.against, &cli.scan) { (Some(d), None) => Viewer::open_against(&cli.path, d, cli.size, cli.preset), (None, Some(s)) => Viewer::open_scan(&cli.path, s, cli.size, cli.preset), (Some(_), Some(_)) => Err("--against and --scan are exclusive".to_string()), (None, None) if std::path::Path::new(&cli.path).extension().is_some_and(|e| e.eq_ignore_ascii_case(vug_view::deviation::REPORT_EXT)) => Viewer::open_report(&cli.path, cli.size, cli.preset), (None, None) => Viewer::open_with(&cli.path, cli.size, cli.preset, prefs) }; // VUGFACEDEVIATION (SR525): a scan against its design; VUGFACEDEVIATIONSOLIDFAULT (SR565): a .vug part with --scan opens as the journal; VUGFACECOMMANDCARET (SR587): a .vugreport opens as the report sheet with no map
    let mut viewer = match opened {
        Ok(v) => v,
        Err(e) => {
            eprintln!("vug-view: {e}");
            std::process::exit(1);
        }
    };
    // VUGPHOTOPREFFACEWIRE (SR588): each `--camera` / `--pp` line runs photo::command (Principia's store, the open photo's prefs) BEFORE `--side`, a refusal exits 1.
    for l in &cli.pref {
        if let Err(e) = vug_view::photo::command(&mut viewer.view, l) {
            eprintln!("vug-view: {e}");
            std::process::exit(1);
        }
    }
    // VUGFACEPHOTOSIDE (SR547): `--side <path>` measures the side photo against the opened one; a refusal is named.
    if let Some(p) = &cli.side {
        if let Err(e) = vug_view::photo::side_path(&mut viewer.view, p) {
            eprintln!("vug-view: {e}");
        }
    }
    // VUGFACEGDT (SR553): each `--gdt` names a callout on the map (deviation::command), a refusal named before any frame.
    for g in &cli.gdt {
        if let Err(e) = vug_view::deviation::command(&mut viewer.view, &viewer.model, &format!("gdt {g}")) {
            eprintln!("vug-view: {e}");
            std::process::exit(1);
        }
    }
    // VUGFACEGDTREMEASURE (SR560): `--datum` names the frame position reads in (deviation::command), a refusal exits 1.
    if let Some(d) = &cli.datum {
        if let Err(e) = vug_view::deviation::command(&mut viewer.view, &viewer.model, &format!("datum {d}")) {
            eprintln!("vug-view: {e}");
            std::process::exit(1);
        }
    }
    // VUGFACEGDTREPORTKEY (SR575): `--report` writes the verdicts through the map's `report` command (a refusal exits 1);
    // with no window asked (no --out, no window backend) the run ends there, else the frame or the window follows.
    if cli.report {
        match viewer.report_command() {
            Ok(p) => println!("report -> {}", p.display()),
            Err(e) => {
                eprintln!("vug-view: {e}");
                std::process::exit(1);
            }
        }
        if cli.out.is_none() && cfg!(not(target_os = "macos")) {
            std::process::exit(0);
        }
    }
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
