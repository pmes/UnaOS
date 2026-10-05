
use clap::{Parser, Subcommand};
use anyhow::Result;
use aether::*;

#[derive(Parser)]
#[command(name = "aether")]
#[command(about = "Aether Web Browser Engine Handler")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Open a URL (Handler mode)
    Open {
        url: String,
    },
    /// Headless render: fetch (or read --html), render once, write PNG + ledger, exit
    Render {
        /// URL to load (ignored when --html is given)
        url: Option<String>,
        /// Load this local HTML file instead of fetching a URL
        #[arg(long)]
        html: Option<std::path::PathBuf>,
        /// PNG output path
        #[arg(long, default_value = "aether-render.png")]
        out: std::path::PathBuf,
        /// Ledger dump path
        #[arg(long, default_value = "aether-ledger.txt")]
        ledger: std::path::PathBuf,
        /// Scroll offset (px) before rendering — audit below the fold
        #[arg(long, default_value_t = 0.0)]
        scroll: f64,
        /// Viewport width
        #[arg(long, default_value_t = 800)]
        width: u32,
        /// Viewport height
        #[arg(long, default_value_t = 600)]
        height: u32,
    },
}

use bandy::synapse::Synapse;
use bandy::signals::SMessage;

pub async fn ignite(synapse: Synapse) -> Result<()> {
    let mut rx = synapse.subscribe();
    println!("Aether ignited. Listening for OpenDocument messages...");

    let mut engine = AetherEngine::new();

    loop {
        tokio::select! {
            Ok(msg) = rx.recv() => {
                match msg {
                    SMessage::OpenDocument { url } => {
                        println!("Received OpenDocument for {}", url);
                        if let Err(e) = engine.load_url(&url).await {
                            eprintln!("Failed to load url {}: {}", url, e);
                        }
                    }
                    // Stria's replies for this page's <video>/<audio> elements.
                    ref m @ (SMessage::MediaOpened { .. }
                    | SMessage::MediaFrame { .. }
                    | SMessage::MediaEnded { .. }
                    | SMessage::MediaError { .. }) => {
                        engine.on_media_message(m);
                    }
                    _ => {}
                }
                // Charter: Aether renders, Stria plays. The page's media elements queued
                // their requests (posters on layout; muted autoplay; never bare autoplay —
                // Chromium's policy): hand them to Stria.
                for req in engine.take_media_requests() {
                    synapse.fire(req);
                }
            }
            // Engine tick
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(16)), if engine.document.is_some() => {
                // engine.tick() — NOT a bare run_jobs(): page timers live in
                // the event_loop timer queue now, and only tick() fires them
                // (bounded to one generation per turn).
                // An http(s) media source that finished caching releases its request here
                // (AUDIOTRACK M4), even when no bus message arrived this turn.
                for req in engine.take_media_requests() {
                    synapse.fire(req);
                }
                if engine.tick() {
                    // We let the shells handle repaint, so the handler mode doesn't need to do surface blits here,
                    // but we will keep this loop alive for JS timers/jobs.
                    // If we need to send a frame back through bandy in the future, we do it here.
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Open { url } => {
            let synapse = Synapse::new();
            
            let syn_clone = synapse.clone();
            let u = url.clone();
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                syn_clone.fire(SMessage::OpenDocument { url: u });
            });
            
            ignite(synapse).await?;
        }
        Commands::Render { url, html, out, ledger, scroll, width, height } => {
            let (w, h, missing) = aether::headless::render_headless_opts(
                url.as_deref(), html.as_deref(), out, ledger, *scroll, *width, *height,
            ).await?;
            println!(
                "rendered {}x{} -> {} | {} missing APIs -> {}",
                w, h, out.display(), missing, ledger.display()
            );
        }
    }

    Ok(())
}

