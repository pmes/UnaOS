// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `facet-view` — the UnaOS image vessel: a window over the Facet handler.
//!
//! Like every vessel this binary is wiring and lifecycle only. It starts a Synapse, serves the
//! Images handler on it (`facet::serve`), and runs the [`facet_view::Viewer`] controller between
//! the window and Facet: window input becomes Facet requests, Facet's rendered frames become
//! `SurfaceBlit`s. On macOS the window is quartzite's image surface (`bootstrap_image_surface`,
//! which forwards keys, wheel and resizes over the bus); everywhere, the headless witness drives
//! the very same controller from a key script and writes the last frame as a PNG:
//!
//! ```text
//! facet-view <image>                                    # the window (macOS)
//! facet-view <image> --size 320x240 --keys "+r]x" --out shot.png   # headless witness
//! ```
//! Key script: one character per shortcut; `{Up}` `{Down}` `{Left}` `{Right}` for the arrows.

use bandy::signals::FacetCommand;
use bandy::{Origin, SMessage, Synapse};
use facet_view::Viewer;
use std::time::Duration;

struct Cli {
    path: String,
    size: (u32, u32),
    keys: Option<String>,
    out: Option<String>,
}

fn cli() -> Cli {
    let mut a = std::env::args().skip(1);
    let mut c = Cli { path: String::new(), size: (800, 600), keys: None, out: None };
    while let Some(arg) = a.next() {
        match arg.as_str() {
            "--size" => {
                let v = a.next().unwrap_or_default();
                let (w, h) = v.split_once('x').unwrap_or(("", ""));
                c.size = (w.parse().unwrap_or(800), h.parse().unwrap_or(600));
            }
            "--keys" => c.keys = a.next(),
            "--out" => c.out = a.next(),
            "--help" | "-h" => {
                eprintln!("usage: facet-view <image> [--size WxH] [--keys SCRIPT] [--out shot.png]");
                for (k, what) in facet_view::SHORTCUTS {
                    eprintln!("  {k:<8} {what}");
                }
                std::process::exit(0);
            }
            _ => c.path = arg,
        }
    }
    if c.path.is_empty() {
        eprintln!("facet-view: usage: facet-view <image> [--size WxH] [--keys SCRIPT] [--out shot.png]");
        std::process::exit(2);
    }
    c
}

/// Split a key script into keys: single characters, or `{Name}` tokens.
fn keys(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = script.chars().peekable();
    while let Some(c) = it.next() {
        if c == '{' {
            let name: String = it.by_ref().take_while(|&c| c != '}').collect();
            out.push(name);
        } else {
            out.push(c.to_string());
        }
    }
    out
}

/// Fire `reqs` and settle every answer (and every follow-up it causes) through the viewer.
/// Returns the newest frame the viewer asked to blit.
async fn settle(
    viewer: &mut Viewer,
    synapse: &Synapse,
    rx: &mut tokio::sync::broadcast::Receiver<SMessage>,
    mut reqs: Vec<FacetCommand>,
) -> Option<SMessage> {
    let mut blit = None;
    while !reqs.is_empty() {
        let mut pending: Vec<u64> = reqs.iter().filter_map(|r| r.receipt_id()).collect();
        for r in reqs.drain(..) {
            synapse.fire(SMessage::Facet(r));
        }
        while !pending.is_empty() {
            let msg = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await;
            let Ok(Ok(SMessage::Facet(cmd))) = msg else {
                if msg.is_err() {
                    eprintln!("facet-view: Facet did not answer within 30 s");
                    return blit;
                }
                continue;
            };
            if let Some(r) = cmd.receipt_id().filter(|_| !cmd.is_request()) {
                pending.retain(|p| *p != r);
            }
            let (more, b) = viewer.answer(&cmd);
            reqs.extend(more);
            if b.is_some() {
                blit = b;
            }
        }
    }
    blit
}

fn main() {
    let cli = cli();
    let principal = Origin::LocalUser(std::env::var("USER").unwrap_or_else(|_| "una".into()));
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    let synapse = Synapse::new();
    {
        let served = synapse.subscribe();
        rt.spawn(facet::serve(synapse.clone(), served, facet::Facet::new()));
    }

    // ---- The headless witness: the same controller, the same bus, frames to a PNG. ----
    if cli.out.is_some() || cfg!(not(target_os = "macos")) {
        let Some(out) = cli.out.clone() else {
            eprintln!("facet-view: no native window backend on this platform yet (macOS first); use --out");
            std::process::exit(2);
        };
        let code = rt.block_on(async {
            let mut rx = synapse.subscribe();
            let mut v = Viewer::new(&cli.path, principal, cli.size);
            let start = v.start();
            let mut frame = settle(&mut v, &synapse, &mut rx, start).await;
            for k in keys(cli.keys.as_deref().unwrap_or("")) {
                let reqs = v.key(&k);
                if let Some(f) = settle(&mut v, &synapse, &mut rx, reqs).await {
                    frame = Some(f);
                }
            }
            if let Some(e) = &v.last_error {
                eprintln!("facet-view: {e}");
            }
            match frame {
                Some(SMessage::SurfaceBlit { width, height, pixels, .. }) => {
                    if let Err(e) = std::fs::write(&out, facet::png::encode(width, height, &pixels)) {
                        eprintln!("facet-view: {out}: {e}");
                        return 1;
                    }
                    println!("{} -> {out}", v.title());
                    0
                }
                _ => {
                    eprintln!("facet-view: no frame rendered");
                    1
                }
            }
        });
        std::process::exit(code);
    }

    // ---- The window (macOS AppKit via quartzite's image surface). ----
    #[cfg(target_os = "macos")]
    {
        use gneiss_pal::paths::UnaPaths;
        UnaPaths::awaken().expect("CRITICAL: Failed to awaken spatial paths");
        bandy::telemetry::ignite(UnaPaths::root().join("logs"));
        log::info!("[FACET-VIEW] :: boot — {}", cli.path);

        // The controller loop: window input and Facet answers in, requests and blits out.
        {
            let synapse = synapse.clone();
            let mut rx = synapse.subscribe();
            let mut v = Viewer::new(&cli.path, principal, cli.size);
            rt.spawn(async move {
                for r in v.start() {
                    synapse.fire(SMessage::Facet(r));
                }
                loop {
                    match rx.recv().await {
                        Ok(SMessage::Facet(cmd)) => {
                            let (more, blit) = v.answer(&cmd);
                            for r in more {
                                synapse.fire(SMessage::Facet(r));
                            }
                            if let Some(b) = blit {
                                synapse.fire(b);
                            }
                            if let Some(e) = v.last_error.take() {
                                log::warn!("[FACET-VIEW] :: {e}");
                            }
                        }
                        Ok(msg) => {
                            for r in v.input(&msg) {
                                synapse.fire(SMessage::Facet(r));
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }

        let title = std::path::Path::new(&cli.path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("facet")
            .to_string();
        let surface_synapse = synapse.clone();
        quartzite::Backend::new_vessel(
            "org.unaos.facet",
            &format!("facet — {title}"),
            (cli.size.0 as f64, cli.size.1 as f64),
            move |_window| {
                quartzite::platforms::macos::image_view::bootstrap_image_surface(facet_view::SURFACE, surface_synapse.clone())
            },
        )
        .run();
        drop(rt);
    }
}
