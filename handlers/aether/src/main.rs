
pub mod net;
pub mod dom;
pub mod layout;
pub mod render;
pub mod js;
pub mod images;
pub mod forms;
pub mod css;

pub mod ledger;
pub mod storage;
pub mod workers;
pub mod event_loop;
pub mod fonts;
pub mod api;
use clap::{Parser, Subcommand};
use anyhow::Result;
use aether::AetherEngine;

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
                    _ => {}
                }
            }
            // M1 Mock layout tick
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(16)), if engine.document.is_some() => {
                if let Some(js) = &mut engine.js_engine {
                    let _ = js.context.run_jobs();
                }
                
                if let Some(layout) = &mut engine.layout_tree {
                    layout.mark_dirty();
                    if layout.dirty {
                        if let Some(doc) = &engine.document {
                            layout.recompute(doc);
                        }
                        
                        // For the standalone mock, we just generate a surface blit here if needed
                        engine.needs_repaint = true;
                    }
                }
                
                if engine.needs_repaint {
                    let w = 800;
                    let h = 600;
                    let mut buf = vec![0; (w * h * 4) as usize];
                    engine.render_frame(&mut buf, w, h);
                    if let Some(url) = &engine.document.as_ref().and_then(|d| d.children().next()).map(|_| "url") { // Mock URL fetch
                        // mock publish
                        synapse.fire(SMessage::SurfaceBlit {
                            url: url.to_string(),
                            width: w,
                            height: h,
                            pixels: buf,
                        });
                    }
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
    }

    Ok(())
}

