
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
            // Engine tick
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(16)), if engine.document.is_some() => {
                if let Some(js) = &mut engine.js_engine {
                    let _ = js.context.run_jobs();
                }
                
                if engine.needs_repaint {
                    // We let the shells handle repaint, so the handler mode doesn't need to do surface blits here,
                    // but we will keep this loop alive for JS timers/jobs.
                    // If we need to send a frame back through bandy in the future, we do it here.
                    engine.needs_repaint = false;
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

